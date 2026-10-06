//! The reader: a document back into the model.
//!
//! The other return path, and not the one the interpreter is. [`super::perform`]
//! turns a model into sound; this turns a *document* into a model -- which is
//! what a score opened from typed text needs before any of the model's verbs can
//! touch it. A score typed as ABC, imported from MusicXML or written by hand is
//! a document and nothing else until something reads one.
//!
//! **There is one input format, not four.** The engraver normalizes whatever it
//! loaded to MEI ([`super::Engraver::mei`]), so a caller hands this the
//! normalized document and every importer verovio has is covered by reading one
//! encoding. That is also why the reader is here rather than beside a parser
//! per format.
//!
//! **What it must not do is lose what it cannot hold.** The model grew where a
//! document is musical -- the header, the barlines, the breaks and the beams a
//! writer chose -- so what is left outside it is what the engraver recomputes
//! when nobody chose: automatic beaming, the line breaks that merely fit, the
//! staff geometry. Those are not read and are not loss. Anything else a
//! document carries and this cannot represent is a **gap to write down**, and
//! the plan says so rather than the reader swallowing it.
//!
//! **Ids are how a page names a note**, so they survive: an element written by
//! this layer carries the model's own id (`n7`, and `n7-2` for a part of one
//! split across a barline), and reading it back recovers the item -- the split
//! parts rejoin into the one item they came from, which is what makes a sheet
//! written out and read back the sheet that was written. A document from
//! anywhere else has ids of its own shape; those are dropped and fresh ones
//! minted, because an id is only meaningful inside the model that minted it.

use std::collections::{BTreeMap, HashMap};

use roxmltree::{Document, Node};

use super::key_alteration;
use super::model::{Grid, Header, Item, Marks, Meter, Pitch, Sheet, Spanner, Staff, Step, Voice};
use crate::ratio::Ratio;

/// Read a normalized MEI document into the score model.
///
/// # Errors
/// When the document is not readable XML, or carries no `<score>` -- the two
/// cases where there is nothing to read rather than something to skip.
pub fn mei_to_sheet(mei: &str) -> Result<Sheet, String> {
    let doc = Document::parse(mei).map_err(|e| format!("the document is not readable XML: {e}"))?;
    let score = find(doc.root_element(), "score")
        .ok_or_else(|| "the document carries no <score> to read".to_string())?;

    let mut reader = Reader {
        sheet: Sheet {
            header: read_header(doc.root_element()),
            ..Sheet::default()
        },
        ..Reader::default()
    };
    for child in score.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "scoreDef" => {
                if reader.staves.is_empty() {
                    reader.staves = read_staves(child);
                    reader.sheet.groups = read_groups(child, reader.staves.len());
                    reader.sheet.key = read_key(child);
                    reader.key = reader.sheet.key.clone();
                    reader.sheet.page = read_page(child);
                    // the page's own text, where this layer wrote it: the
                    // fields the document head has no place for, and where
                    // each one sits
                    super::pagetext::read_running(child, &mut reader.sheet.header);
                }
                if let Some(meter) = read_meter(child, reader.measure) {
                    reader.meters.push(meter);
                }
            }
            "section" => {
                if child.attribute("type") == Some("multirests") {
                    reader.sheet.grid.multirests = true;
                }
                for node in child.children().filter(Node::is_element) {
                    reader.section_child(node);
                }
            }
            _ => {}
        }
    }
    Ok(reader.finish())
}

/// A document being read: the sheet so far, and what is collected on the way
/// to be put back once every item has its id.
#[derive(Default)]
struct Reader {
    sheet: Sheet,
    staves: Vec<Staff>,
    meters: Vec<Meter>,
    /// The key in force where the reading is.
    key: String,
    // [staff][voice] -> the items read so far, appended measure by measure.
    content: Vec<Vec<Vec<Item>>>,
    beams: Vec<(usize, usize, usize, usize)>,
    ftrems: Vec<(usize, usize, usize)>,
    clefs: Vec<(usize, usize, String)>,
    ties: Vec<(String, String)>,
    attached: Vec<Attached>,
    pending_break: Option<String>,
    measure: usize,
    /// Where each voice's last measure is in its items, which a measure
    /// written as a repeat of it copies.
    last: HashMap<(usize, usize), (usize, usize)>,
    /// The measure each `<measure>` starts at, in the order they are
    /// written: a numbered rest is one of them and several measures.
    elements: Vec<usize>,
}

/// Something that hangs off a measure, by the element it starts at -- or,
/// where it names none, by its beat of the measure it is written in.
struct Attached {
    name: String,
    start: Option<String>,
    end: Option<String>,
    /// Which `<measure>` it is written in, counted as they are written.
    element: usize,
    text: String,
    attrs: BTreeMap<String, String>,
}

impl Reader {
    fn section_child(&mut self, node: Node) {
        match node.tag_name().name() {
            "pb" => self.pending_break = Some("page".to_string()),
            "sb" => self.pending_break = Some("system".to_string()),
            "scoreDef" => {
                if let Some(meter) = read_meter(node, self.measure) {
                    self.meters.push(meter);
                }
                if let Some(key) = read_key_change(node) {
                    if self.measure == 0 {
                        self.sheet.key = key.clone();
                    } else {
                        self.sheet.grid.keys.push((self.measure, key.clone()));
                    }
                    self.key = key;
                }
            }
            "ending" => {
                let first = self.measure;
                for inner in node.children().filter(Node::is_element) {
                    self.section_child(inner);
                }
                if self.measure > first {
                    let label = node.attribute("n").unwrap_or("1").to_string();
                    self.sheet
                        .grid
                        .endings
                        .push((first, self.measure - 1, label));
                }
            }
            "measure" => self.read_measure(node),
            _ => {}
        }
    }

    /// One measure: its layers appended to the content, and what hangs off it
    /// collected for later.
    fn read_measure(&mut self, measure: Node) {
        if let Some(kind) = self.pending_break.take() {
            self.sheet.grid.breaks.push((self.measure, kind));
        }
        let element = self.elements.len();
        self.elements.push(self.measure);
        let mut span = 1i64;
        let mut repeated = false;
        for staff in measure.children().filter(|n| n.has_tag_name("staff")) {
            // An accidental holds for the rest of its measure, at its own step
            // and octave, on this staff. A new measure starts again from the
            // armature -- the ordinary convention, and the one the emitter
            // writes with, so the two have to agree or a score means something
            // different after a save.
            let mut in_force: HashMap<(i32, i32), i32> = HashMap::new();
            let si = number(staff, 1) - 1;
            while self.content.len() <= si {
                self.content.push(Vec::new());
            }
            for layer in staff.children().filter(|n| n.has_tag_name("layer")) {
                let vi = number(layer, 1) - 1;
                while self.content[si].len() <= vi {
                    self.content[si].push(Vec::new());
                }
                let start = self.content[si][vi].len();
                // **A measure drawn as a repeat** holds what the one before
                // held: its items again, as new items.
                if layer.children().any(|n| n.has_tag_name("mRpt")) {
                    repeated = true;
                    if let Some(&(from, to)) = self.last.get(&(si, vi)) {
                        let copies: Vec<Item> = self.content[si][vi][from..to]
                            .iter()
                            .map(|item| item.with_id(0))
                            .collect();
                        self.content[si][vi].extend(copies);
                    }
                } else {
                    let mut facts = LayerFacts::default();
                    read_items(
                        layer,
                        Ratio::ONE,
                        &self.key,
                        &mut in_force,
                        &mut self.content[si][vi],
                        &mut facts,
                    );
                    self.beams
                        .extend(facts.beams.into_iter().map(|(a, b)| (si, vi, a, b)));
                    self.ftrems
                        .extend(facts.ftrems.into_iter().map(|a| (si, vi, a)));
                    if vi == 0 {
                        self.clefs
                            .extend(facts.clefs.into_iter().map(|(at, clef)| (si, at, clef)));
                    }
                    if let Some(num) = facts.multirest {
                        span = span.max(num);
                        self.sheet.grid.multirests = true;
                    }
                }
                let end = self.content[si][vi].len();
                self.last.insert((si, vi), (start, end));
            }
        }
        if repeated {
            self.sheet.grid.repeats.push(self.measure);
        }
        let last = self.measure + span.max(1) as usize - 1;
        if let Some(right) = measure.attribute("right")
            && right != "single"
        {
            self.sheet.grid.barlines.push((last, right.to_string()));
        }
        for node in measure.children().filter(Node::is_element) {
            let name = node.tag_name().name();
            if name == "staff" {
                continue;
            }
            if name == "repeatMark" {
                let kind = match (node.attribute("func"), node.attribute("label")) {
                    (_, Some("tocoda")) => Some("tocoda"),
                    (Some("segno"), _) => Some("segno"),
                    (Some("coda"), _) => Some("coda"),
                    (Some("fine"), _) => Some("fine"),
                    (Some("daCapo"), _) => Some("dacapo"),
                    (Some("dalSegno"), _) => Some("dalsegno"),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.sheet.grid.marks.push((self.measure, kind.to_string()));
                }
                continue;
            }
            let start = node.attribute("startid").map(strip_hash);
            let end = node.attribute("endid").map(strip_hash);
            if name == "tie" {
                if let (Some(start), Some(end)) = (start, end) {
                    self.ties.push((start, end));
                }
                continue;
            }
            if start.is_none() && node.attribute("tstamp").is_none() {
                continue;
            }
            self.attached.push(Attached {
                name: name.to_string(),
                start,
                end,
                element,
                text: text_in(node),
                attrs: node
                    .attributes()
                    .map(|a| (a.name().to_string(), a.value().to_string()))
                    .collect(),
            });
        }
        self.measure = last + 1;
    }

    fn finish(mut self) -> Sheet {
        let mut sheet = std::mem::take(&mut self.sheet);
        sheet.grid.meters = if self.meters.is_empty() {
            Grid::default().meters
        } else {
            std::mem::take(&mut self.meters)
        };
        // The last measure's barline is `end` by default and the emitter
        // writes it unasked, so keeping it would make every read-back carry an
        // override nobody wrote.
        if let Some(last) = self.measure.checked_sub(1) {
            sheet
                .grid
                .barlines
                .retain(|(m, kind)| !(*m == last && kind == "end"));
        }

        let content = std::mem::take(&mut self.content);
        sheet.staves = content
            .into_iter()
            .enumerate()
            .map(|(si, voices)| Staff {
                voices: voices.into_iter().map(|items| Voice { items }).collect(),
                ..self.staves.get(si).cloned().unwrap_or_default()
            })
            .collect();
        if sheet.staves.is_empty() {
            sheet.staves = vec![self.staves.first().cloned().unwrap_or_default()];
        }

        // The order matters, and each step is where it is for a reason. The
        // emitter's padding goes first, while "it has no id" still identifies
        // it and before anything has renumbered; it is always trailing, so
        // dropping it moves no position anything else holds. Then ids, so a
        // beam read as a run of *positions* can name its ends before rejoining
        // the split parts moves them.
        drop_padding(&mut sheet);
        sheet.assign_ids();
        apply_beams(&mut sheet, &self.beams);
        let ftrem_ids: Vec<(u64, u64)> = self
            .ftrems
            .iter()
            .filter_map(|&(si, vi, a)| {
                let items = &sheet.staves.get(si)?.voices.get(vi)?.items;
                Some((items.get(a)?.id(), items.get(a + 1)?.id()))
            })
            .collect();
        let clef_ids: Vec<(usize, u64, String)> = self
            .clefs
            .iter()
            .filter_map(|(si, at, clef)| {
                let item = sheet.staves.get(*si)?.voices.first()?.items.get(*at)?;
                Some((*si, item.id(), clef.clone()))
            })
            .collect();
        rejoin(&mut sheet);
        for (from, to) in ftrem_ids {
            sheet.spanners.push(Spanner {
                kind: "ftrem".to_string(),
                from,
                to,
            });
        }
        for (si, id, clef) in clef_ids {
            let Some(t) = sheet.staves[si].voices.first().and_then(|voice| {
                let index = voice.items.iter().position(|i| i.id() == id)?;
                Some(
                    voice.items[..index]
                        .iter()
                        .fold(Ratio::ZERO, |acc, i| acc + i.dur()),
                )
            }) else {
                continue;
            };
            sheet.staves[si].clefs.push((t, clef));
        }
        apply_attachments(&mut sheet, &self.attached, &self.elements);
        apply_ties(&mut sheet, &self.ties);
        sheet
    }
}

/// The first descendant with this tag name, at any depth.
fn find<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

/// The text of the first descendant with this tag name, trimmed.
fn text_of(node: Node, name: &str) -> String {
    find(node, name)
        .and_then(|n| n.text())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// What is written above the music.
///
/// Verovio's importers put the title in two different places -- `<titleStmt>`,
/// which is where this layer writes it, and `<workList>`, which is where the
/// ABC importer puts it -- so both are read and the first non-empty one wins.
/// A document that says it in neither is untitled, which is a state and not a
/// failure.
fn read_header(root: Node) -> Header {
    let head = match find(root, "meiHead") {
        None => return Header::default(),
        Some(head) => head,
    };
    let titles: Vec<Node> = head
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "title")
        .collect();
    let named = |kind: &str| -> String {
        titles
            .iter()
            .find(|n| n.attribute("type") == Some(kind))
            .and_then(|n| n.text())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let mut title = named("main");
    if title.is_empty() {
        title = titles
            .iter()
            .filter(|n| n.attribute("type").is_none())
            .find_map(|n| n.text().map(str::trim).filter(|t| !t.is_empty()))
            .unwrap_or_default()
            .to_string();
    }
    Header {
        title,
        subtitle: named("subordinate"),
        composer: text_of(head, "composer"),
        lyricist: text_of(head, "lyricist"),
        ..Header::default()
    }
}

/// The staves, in order, as their definitions say them: the clef as the
/// model spells one (`"G2"`), the line count, the names and the
/// transposition.
fn read_staves(score_def: Node) -> Vec<Staff> {
    score_def
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "staffDef")
        .map(|def| {
            let shape = def
                .attribute("clef.shape")
                .map(str::to_string)
                .or_else(|| find(def, "clef")?.attribute("shape").map(str::to_string))
                .unwrap_or_else(|| "G".to_string());
            let line = def
                .attribute("clef.line")
                .map(str::to_string)
                .or_else(|| find(def, "clef")?.attribute("line").map(str::to_string))
                .unwrap_or_else(|| "2".to_string());
            let named = |tag: &str, attr: &str| {
                def.children()
                    .find(|n| n.has_tag_name(tag))
                    .map(text_in)
                    .or_else(|| def.attribute(attr).map(str::to_string))
                    .unwrap_or_default()
            };
            Staff {
                clef: format!("{shape}{line}"),
                voices: Vec::new(),
                lines: def
                    .attribute("lines")
                    .and_then(|n| n.parse().ok())
                    .filter(|n| *n != 5),
                label: named("label", "label"),
                abbr: named("labelAbbr", "label.abbr"),
                transpose: def
                    .attribute("trans.semi")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0),
                ..Staff::default()
            }
        })
        .collect()
}

/// The groups the staves are written in: each inner group, by the staves it
/// spans and the sign at its left. The one group a score always has -- a
/// brace over several staves, or none over one -- is not a choice and reads as
/// none.
fn read_groups(score_def: Node, staves: usize) -> Vec<super::model::Group> {
    let Some(top) = score_def.children().find(|n| n.has_tag_name("staffGrp")) else {
        return Vec::new();
    };
    let defs: Vec<Node> = top
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "staffDef")
        .collect();
    let index = |def: Node| defs.iter().position(|d| *d == def);
    let mut groups = Vec::new();
    for group in top
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "staffGrp")
    {
        let inner: Vec<usize> = group
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "staffDef")
            .filter_map(index)
            .collect();
        let (Some(&first), Some(&last)) = (inner.first(), inner.last()) else {
            continue;
        };
        let symbol = group.attribute("symbol").map(str::to_string).or_else(|| {
            find(group, "grpSym")?
                .attribute("symbol")
                .map(str::to_string)
        });
        let Some(symbol) = symbol else { continue };
        let default = group == top && symbol == "brace" && first == 0 && last + 1 == staves;
        if !default {
            groups.push(super::model::Group {
                first,
                last,
                symbol,
            });
        }
    }
    // the outer group alone, over every staff and as the score has it by
    // default, is no choice
    let nested = top
        .descendants()
        .skip(1)
        .any(|n| n.is_element() && n.tag_name().name() == "staffGrp");
    if !nested && groups.len() == 1 && groups[0].symbol == "brace" {
        groups.clear();
    }
    groups
}

/// A change of key a score definition inside the music states, if it states
/// one.
fn read_key_change(score_def: Node) -> Option<String> {
    let states = score_def.attribute("key.sig").is_some()
        || score_def.attribute("keysig").is_some()
        || find(score_def, "keySig").is_some();
    states.then(|| read_key(score_def))
}

/// The key, as the tonic name the model holds.
///
/// Written `key.sig` by this layer and normalized to `keysig` by verovio, so
/// both spellings are read -- which is the sort of difference that is invisible
/// until a document makes a round trip through the engraver.
fn read_key(score_def: Node) -> String {
    let sig = score_def
        .attribute("key.sig")
        .or_else(|| score_def.attribute("keysig"))
        .or_else(|| find(score_def, "keySig")?.attribute("sig"))
        .unwrap_or("0");
    const NAMES: [(&str, &str); 15] = [
        ("0", "C"),
        ("1s", "G"),
        ("2s", "D"),
        ("3s", "A"),
        ("4s", "E"),
        ("5s", "B"),
        ("6s", "F#"),
        ("7s", "C#"),
        ("1f", "F"),
        ("2f", "Bb"),
        ("3f", "Eb"),
        ("4f", "Ab"),
        ("5f", "Db"),
        ("6f", "Gb"),
        ("7f", "Cb"),
    ];
    NAMES
        .iter()
        .find(|(s, _)| *s == sig)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| "C".to_string())
}

/// The page setup this `scoreDef` states, if it states a page: its size, the
/// margins it gives (the default where it gives none) and the staff size, read
/// from millimetres to the tenth -- what [`super::mei`] writes.
fn read_page(def: Node) -> Option<super::PageSetup> {
    let mm = |name: &str| -> Option<f64> {
        def.attribute(name)?
            .trim()
            .strip_suffix("mm")?
            .trim()
            .parse()
            .ok()
    };
    let tenths = |name: &str| mm(name).map(|v| (v * 10.0).round() as u32);
    let default = super::PageSetup::default();
    let setup = super::PageSetup {
        width: tenths("page.width")?,
        height: tenths("page.height")?,
        margins: [
            tenths("page.topmar").unwrap_or(default.margins[0]),
            tenths("page.rightmar").unwrap_or(default.margins[1]),
            tenths("page.botmar").unwrap_or(default.margins[2]),
            tenths("page.leftmar").unwrap_or(default.margins[3]),
        ],
        staff: mm("vu.height")
            .map(|v| (v * 800.0).round() as u32)
            .unwrap_or(default.staff),
    };
    setup.check().ok().map(|()| setup)
}

/// The meter this `scoreDef` states, if it states one.
fn read_meter(score_def: Node, measure: usize) -> Option<Meter> {
    let count = score_def
        .attribute("meter.count")
        .or_else(|| find(score_def, "meterSig")?.attribute("count"))?
        .parse()
        .ok()?;
    let unit = score_def
        .attribute("meter.unit")
        .or_else(|| find(score_def, "meterSig")?.attribute("unit"))?
        .parse()
        .ok()?;
    Some(Meter {
        measure,
        count,
        unit,
    })
}

/// What a layer says beside its items, by their positions in the voice: the
/// beams and the two-note tremolos around them, the changes of clef before
/// them, and how many measures a numbered rest stands for.
#[derive(Default)]
struct LayerFacts {
    beams: Vec<(usize, usize)>,
    ftrems: Vec<usize>,
    clefs: Vec<(usize, String)>,
    multirest: Option<i64>,
}

/// One layer's items, descending through the containers that are not items.
///
/// `scale` is what a surrounding tuplet does to every written value inside it,
/// which is how a triplet eighth comes back as `1/12` rather than `1/8`.
fn read_items(
    node: Node,
    scale: Ratio,
    key: &str,
    in_force: &mut HashMap<(i32, i32), i32>,
    out: &mut Vec<Item>,
    facts: &mut LayerFacts,
) {
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            // Containers: a beam is read from the elements it wraps (the
            // spanner is rebuilt in `apply_attachments`), a tuplet scales them.
            "beam" => {
                // The container is not an item; what it says is that the
                // elements inside it are beamed together, which the model holds
                // as a spanner over the first and the last of them.
                let first = out.len();
                read_items(child, scale, key, in_force, out, facts);
                if out.len() > first {
                    facts.beams.push((first, out.len() - 1));
                }
            }
            // A tremolo on one note: the note inside, its strokes the value
            // each one halves.
            "bTrem" => {
                let first = out.len();
                read_items(child, scale, key, in_force, out, facts);
                let strokes = match child.attribute("unitdur") {
                    Some("8") => 1,
                    Some("16") => 2,
                    Some("32") => 3,
                    _ => find(child, "note")
                        .and_then(|n| n.attribute("stem.mod"))
                        .and_then(|m| m.chars().next()?.to_digit(10))
                        .unwrap_or(1) as u8,
                };
                for item in &mut out[first..] {
                    if let Item::Note { marks, .. } = item {
                        marks.tremolo = Some(strokes);
                    }
                }
            }
            // Two notes alternating: each written with the value of the two
            // together, which is MEI's spelling and not the model's.
            "fTrem" => {
                let first = out.len();
                read_items(child, scale, key, in_force, out, facts);
                for item in &mut out[first..] {
                    *item = item.with_dur(item.dur() / Ratio::from(2));
                }
                if out.len() == first + 2 {
                    facts.ftrems.push(first);
                }
            }
            "tuplet" => {
                let num: i64 = child.attribute("num").unwrap_or("3").parse().unwrap_or(3);
                let numbase: i64 = child
                    .attribute("numbase")
                    .unwrap_or("2")
                    .parse()
                    .unwrap_or(2);
                read_items(
                    child,
                    scale * Ratio::new(numbase, num),
                    key,
                    in_force,
                    out,
                    facts,
                );
            }
            "chord" => {
                let dur = duration(child, scale).unwrap_or(Ratio::new(1, 4));
                let pitches: Vec<Pitch> = child
                    .children()
                    .filter(|n| n.has_tag_name("note"))
                    .filter_map(|n| pitch_of(n, key, in_force))
                    .collect();
                let mut marks = marks_of(child);
                // the lyrics stand on the chord's first note
                if marks.lyrics.is_empty()
                    && let Some(note) = child.children().find(|n| n.has_tag_name("note"))
                {
                    marks.lyrics = lyrics_of(note);
                }
                out.push(Item::Note {
                    id: id_of(child),
                    pitches,
                    dur,
                    tie: false,
                    marks,
                });
            }
            // A beat drawn as a repeat of the one before holds what it repeats.
            "beatRpt" => {
                let id = id_of(child);
                let copy = match out.last() {
                    Some(Item::Note {
                        pitches,
                        dur,
                        marks,
                        ..
                    }) => Item::Note {
                        id,
                        pitches: pitches.clone(),
                        dur: *dur,
                        tie: false,
                        marks: Marks {
                            beat_repeat: true,
                            ..marks.clone()
                        },
                    },
                    Some(other) => other.with_id(id),
                    None => Item::Rest {
                        id,
                        dur: Ratio::new(1, 4),
                    },
                };
                out.push(copy);
            }
            // A numbered rest stands for several measures, sized once the grid
            // is known.
            "multiRest" => {
                let num: i64 = child
                    .attribute("num")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                facts.multirest = Some(num);
                out.push(Item::Rest {
                    id: id_of(child),
                    dur: Ratio::new(-num, 1),
                });
            }
            "clef" => {
                let shape = child.attribute("shape").unwrap_or("G");
                let line = child.attribute("line").unwrap_or("2");
                facts.clefs.push((out.len(), format!("{shape}{line}")));
            }
            "note" => {
                let dur = duration(child, scale).unwrap_or(Ratio::new(1, 4));
                let Some(pitch) = pitch_of(child, key, in_force) else {
                    continue;
                };
                out.push(Item::Note {
                    id: id_of(child),
                    pitches: vec![pitch],
                    dur,
                    tie: child.attribute("tie").is_some_and(|t| t != "t"),
                    marks: marks_of(child),
                });
            }
            "rest" | "space" => out.push(Item::Rest {
                id: id_of(child),
                dur: duration(child, scale).unwrap_or(Ratio::new(1, 4)),
            }),
            // A measure rest is as long as its measure, which the layer does not
            // know; `fill_measure_rests` sizes it once the grid is built.
            "mRest" => out.push(Item::Rest {
                id: id_of(child),
                dur: Ratio::ZERO,
            }),
            _ => {}
        }
    }
}

/// The written value: `@dur` and `@dots`, scaled by any tuplet around it.
fn duration(node: Node, scale: Ratio) -> Option<Ratio> {
    let dur = node.attribute("dur")?;
    let base = match dur {
        "breve" => Ratio::new(2, 1),
        "long" => Ratio::new(4, 1),
        n => Ratio::new(1, n.parse::<i64>().ok()?),
    };
    let dots: u32 = node.attribute("dots").unwrap_or("0").parse().unwrap_or(0);
    // Each dot adds half of what came before: 1 + 1/2 + 1/4 + ...
    let mut total = base;
    let mut add = base;
    for _ in 0..dots {
        add = add / Ratio::new(2, 1);
        total = total + add;
    }
    Some(total * scale)
}

/// The written pitch, spelling included.
///
/// A **printed** accidental (`<accid>`) and a **sounding** one (`@accid.ges`)
/// are both alterations and only the first is a statement that it must be seen,
/// which is exactly what `forced` holds -- so the distinction the emitter makes
/// survives the trip back.
///
/// **A note with no accidental of its own is not a natural.** It takes what is
/// in force: an accidental printed earlier in this measure at its step and
/// octave, or failing that the key signature's. The emitter writes nothing
/// where the armature already says it -- which is correct engraving -- so a
/// reader that did not apply the armature would turn every B flat in E flat
/// into a B natural, silently, on the first save. This is the same mistake the
/// encoder was once making in the other direction, and it is caught by the same
/// rule read backwards.
fn pitch_of(note: Node, key: &str, in_force: &mut HashMap<(i32, i32), i32>) -> Option<Pitch> {
    let step = match note.attribute("pname")? {
        "c" => Step::C,
        "d" => Step::D,
        "e" => Step::E,
        "f" => Step::F,
        "g" => Step::G,
        "a" => Step::A,
        "b" => Step::B,
        _ => return None,
    };
    let octave = note
        .attribute("oct")
        .and_then(|o| o.parse().ok())
        .unwrap_or(4);
    // Both spellings, and both places. An accidental is an attribute on the
    // note or a child `<accid>` element, and the emitter writes the *sounding*
    // one as a child while verovio hands back attributes -- so a reader that
    // knew only one of the four would lose an alteration depending on which
    // side of the engraver the document came from.
    let child = find(note, "accid");
    let printed = note
        .attribute("accid")
        .or_else(|| child?.attribute("accid"))
        .map(str::to_string);
    let sounding = note
        .attribute("accid.ges")
        .or_else(|| child?.attribute("accid.ges"));
    let here = (step.index(), octave);
    let alter = match printed.as_deref().or(sounding) {
        Some(accid) => {
            let alter = alteration(accid);
            // A printed accidental holds for the rest of the measure; a merely
            // sounding one states this note and says nothing about the next.
            if printed.is_some() {
                in_force.insert(here, alter);
            }
            alter
        }
        None => *in_force.get(&here).unwrap_or(&key_alteration(key, step)),
    };
    Some(Pitch {
        step,
        alter,
        octave,
        forced: printed.is_some(),
    })
}

/// MEI's accidental names, as semitones.
fn alteration(accid: &str) -> i32 {
    match accid {
        "s" => 1,
        "ss" | "x" => 2,
        "f" => -1,
        "ff" => -2,
        _ => 0,
    }
}

/// What one note carries on itself. What hangs off the measure instead -- a
/// dynamic, an ornament -- is added later, by `apply_attachments`.
fn marks_of(node: Node) -> Marks {
    Marks {
        articulations: node
            .children()
            .filter(|n| n.has_tag_name("artic"))
            .filter_map(|n| n.attribute("artic").map(str::to_string))
            .chain(node.attribute("artic").map(str::to_string))
            .collect(),
        stem: node.attribute("stem.dir").map(str::to_string),
        grace: node.attribute("grace").map(str::to_string),
        lyrics: lyrics_of(node),
        ..Marks::default()
    }
}

/// The lyrics an element carries, verse by verse: a syllable continued by a
/// dash into the next ends in `-`.
fn lyrics_of(node: Node) -> Vec<String> {
    let mut verses: Vec<(usize, String)> = node
        .children()
        .filter(|n| n.has_tag_name("verse"))
        .map(|verse| {
            let n = number(verse, 1);
            let syl = find(verse, "syl");
            let text = syl.map(text_in).unwrap_or_default();
            let dash = syl.and_then(|s| s.attribute("con")) == Some("d");
            (n, if dash { format!("{text}-") } else { text })
        })
        .collect();
    verses.sort();
    let mut out = Vec::new();
    for (n, text) in verses {
        while out.len() + 1 < n {
            out.push(String::new());
        }
        out.push(text);
    }
    out
}

/// All the text under an element, however it is nested in `rend`s.
fn text_in(node: Node) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect::<String>()
        .trim()
        .to_string()
}

/// The model id this element was written from, or `0` when it was written
/// somewhere else -- `Sheet::assign_ids` mints those.
fn id_of(node: Node) -> u64 {
    let Some(id) = node.attribute(("http://www.w3.org/XML/1998/namespace", "id")) else {
        return 0;
    };
    let Some(rest) = id.strip_prefix('n') else {
        return 0;
    };
    // `n7` is the item; `n7-2` is a part of it split across a barline.
    rest.split('-').next().unwrap_or("").parse().unwrap_or(0)
}

/// `#n7` -> `n7`.
fn strip_hash(reference: &str) -> String {
    reference.trim_start_matches('#').to_string()
}

/// The `@n` of a staff or layer, 1-based.
fn number(node: Node, default: usize) -> usize {
    node.attribute("n")
        .and_then(|n| n.parse().ok())
        .unwrap_or(default)
}

/// Turn the beams read as runs of positions into the spanners the model holds.
fn apply_beams(sheet: &mut Sheet, beams: &[(usize, usize, usize, usize)]) {
    for &(si, vi, first, last) in beams {
        let Some(voice) = sheet.staves.get(si).and_then(|s| s.voices.get(vi)) else {
            continue;
        };
        let (Some(from), Some(to)) = (voice.items.get(first), voice.items.get(last)) else {
            continue;
        };
        sheet.spanners.push(Spanner {
            kind: "beam".to_string(),
            from: from.id(),
            to: to.id(),
        });
    }
}

/// Drop the rests the **emitter invented**, which are not content.
///
/// A voice is written into whole measures, so a voice that ends mid-bar has its
/// bar completed and a voice shorter than another is padded until the staves
/// are in step. Neither of those rests is in the model, and reading them back
/// would grow the score by a rest on every trip through a document -- the score
/// would gain a bar of silence for having been saved.
///
/// They are known by having **no id**: every element this layer writes from an
/// item carries the item's own, and only what the emitter made up has none. So
/// the rule holds only for a document this layer wrote, which is what `ours`
/// tests -- a document from anywhere else has ids of its own shape, none of them
/// ours, and every rest in it was written by somebody and stays.
fn drop_padding(sheet: &mut Sheet) {
    let ours = sheet
        .voices()
        .flat_map(|v| v.items.iter())
        .any(|i| i.id() != 0);
    if !ours {
        return;
    }
    for voice in sheet.voices_mut() {
        while let Some(last) = voice.items.last() {
            if last.sounds() || last.id() != 0 {
                break;
            }
            voice.items.pop();
        }
    }
}

/// Rejoin the parts a barline split, and size the measure rests.
///
/// The emitter splits an item that overruns a barline and ties the halves, so a
/// document holds two elements where the model held one. Both carry the same
/// model id, which is what lets this put them back -- and putting them back is
/// what makes a sheet written out and read in again the sheet that was written.
fn rejoin(sheet: &mut Sheet) {
    let grid = sheet.grid.clone();
    for staff in &mut sheet.staves {
        for voice in &mut staff.voices {
            // A measure rest was read with no length; give it its measure's.
            let mut at = Ratio::ZERO;
            let mut measure = 0;
            for item in &mut voice.items {
                if item.dur().is_zero() {
                    let (m, _) = grid.position(at);
                    *item = item.with_dur(grid.bar_len(m.max(measure)));
                }
                // a numbered rest: as long as the measures it stands for
                if item.dur() < Ratio::ZERO {
                    let count = (-item.dur()).to_f64().round() as usize;
                    let (m, _) = grid.position(at);
                    let first = m.max(measure);
                    let len =
                        (first..first + count).fold(Ratio::ZERO, |acc, i| acc + grid.bar_len(i));
                    *item = item.with_dur(len);
                }
                at = at + item.dur();
                measure = grid.position(at).0;
            }
            // Then fold each run of same-id items into the one item it was.
            let mut folded: Vec<Item> = Vec::new();
            for item in voice.items.drain(..) {
                match folded.last_mut() {
                    Some(previous) if previous.id() != 0 && previous.id() == item.id() => {
                        let joined = previous.dur() + item.dur();
                        *previous = previous.with_dur(joined);
                    }
                    _ => folded.push(item),
                }
            }
            voice.items = folded;
        }
    }
}

/// Put back what hung off the measures: the spanners, the marks a note
/// carries that MEI writes beside it, and what is written at a point.
///
/// **An end is an element's id, or a beat of a measure**: what is written at
/// an item of a measure drawn as a repeat is written at its beat, the sign
/// standing where the item's element would be, and it comes back on the item
/// the measure holds there.
fn apply_attachments(sheet: &mut Sheet, attached: &[Attached], elements: &[usize]) {
    let ids: BTreeMap<String, u64> = sheet
        .voices()
        .flat_map(|v| v.items.iter())
        .map(|i| (format!("n{}", i.id()), i.id()))
        .collect();
    // a part of an item split across a barline, or a pitch of a chord, names
    // the item
    let resolve = |reference: &str| -> Option<u64> {
        ids.get(reference).copied().or_else(|| {
            let base = reference.split('-').next()?;
            ids.get(base).copied()
        })
    };
    let mut marked: Vec<(u64, &Attached)> = Vec::new();
    let mut pedals: Vec<(u64, bool)> = Vec::new();

    for a in attached {
        let attr = |name: &str| a.attrs.get(name).map(String::as_str);
        let count = |name: &str| attr(name).and_then(|n| n.parse::<usize>().ok());
        let staff = count("staff").map(|n| n.saturating_sub(1));
        let layer = count("layer").map(|n| n.saturating_sub(1));
        let from = match a.start.as_deref() {
            Some(reference) => resolve(reference),
            None => attr("tstamp")
                .and_then(|beat| beat.parse::<f64>().ok())
                .zip(elements.get(a.element))
                .and_then(|(beat, measure)| item_at(sheet, *measure, staff, layer, beat)),
        };
        let Some(from) = from else {
            continue;
        };
        // an end by its beat is in the voice the start is in, unless the
        // element names another
        let to = match a.end.as_deref() {
            Some(reference) => resolve(reference),
            None => attr("tstamp2").and_then(|end| {
                let (across, beat) = match end.split_once("m+") {
                    Some((across, beat)) => (across.trim().parse::<usize>().ok()?, beat),
                    None => (0, end),
                };
                let measure = *elements.get(a.element + across)?;
                let (si, vi, _) = sheet.locate(from)?;
                item_at(
                    sheet,
                    measure,
                    staff.or(Some(si)),
                    layer.or(Some(vi)),
                    beat.trim().parse().ok()?,
                )
            }),
        };
        let spanner = match a.name.as_str() {
            "slur" => Some("slur".to_string()),
            "hairpin" => Some(if attr("form") == Some("dim") {
                "diminuendo".to_string()
            } else {
                "crescendo".to_string()
            }),
            "phrase" => Some("phrase".to_string()),
            "gliss" => Some("gliss".to_string()),
            "bracketSpan" => Some("bracket".to_string()),
            "beamSpan" => Some("beamspan".to_string()),
            "octave" => {
                let below = attr("dis.place") == Some("below");
                Some(
                    match (attr("dis"), below) {
                        (Some("15"), false) => "15ma",
                        (Some("15"), true) => "15mb",
                        (_, true) => "8vb",
                        _ => "8va",
                    }
                    .to_string(),
                )
            }
            "pedal" => {
                pedals.push((from, attr("dir") != Some("up")));
                None
            }
            "tempo" | "dir" | "reh" => {
                sheet.controls.push(super::model::Control {
                    kind: a.name.clone(),
                    on: from,
                    text: a.text.clone(),
                    bpm: attr("midi.bpm").and_then(|b| b.parse().ok()),
                });
                None
            }
            _ => {
                marked.push((from, a));
                None
            }
        };
        if let (Some(kind), Some(to)) = (spanner, to) {
            sheet.spanners.push(Spanner { kind, from, to });
        }
    }
    // the pedal is pressed at one note and let go at a later one: paired in
    // time, whatever order the document wrote the two signs in
    let onsets: HashMap<u64, Ratio> = sheet
        .voices()
        .flat_map(|voice| {
            let mut t = Ratio::ZERO;
            voice.items.iter().map(move |item| {
                let at = t;
                t = t + item.dur();
                (item.id(), at)
            })
        })
        .collect();
    let when = |id: &u64| onsets.get(id).copied().unwrap_or(Ratio::ZERO);
    pedals.sort_by(|a, b| when(&a.0).cmp(&when(&b.0)).then(b.1.cmp(&a.1)));
    let mut down: Option<u64> = None;
    for (id, pressed) in pedals {
        match (pressed, down) {
            (true, _) => down = Some(id),
            (false, Some(from)) => {
                sheet.spanners.push(Spanner {
                    kind: "pedal".to_string(),
                    from,
                    to: id,
                });
                down = None;
            }
            (false, None) => {}
        }
    }

    for (id, a) in marked {
        for voice in sheet.voices_mut() {
            for item in &mut voice.items {
                if item.id() != id {
                    continue;
                }
                let Item::Note { marks, .. } = item else {
                    continue;
                };
                let text = || Some(a.text.clone()).filter(|t| !t.is_empty());
                match a.name.as_str() {
                    "dynam" => marks.dynamic = text(),
                    "trill" | "mordent" | "turn" | "fermata" => {
                        marks.ornament = Some(a.name.clone());
                    }
                    "ornam" => {
                        marks.ornament = a.attrs.get("glyph.name").cloned().or_else(text);
                    }
                    "arpeg" => {
                        marks.arpeggio = Some(
                            a.attrs
                                .get("order")
                                .cloned()
                                .unwrap_or_else(|| "up".to_string()),
                        );
                    }
                    "breath" | "caesura" => marks.breath = Some(a.name.clone()),
                    "lv" => marks.ring = true,
                    "fing" => marks.fingering = text(),
                    "harm" => marks.harmony = text(),
                    _ => {}
                }
            }
        }
    }
}

/// The item sounding at `beat` of `measure` -- MEI's count, in the meter's
/// unit from one -- on a staff, in the voice named or the first that has one
/// there. A beat past what the voice holds of the measure is its last item
/// in it.
fn item_at(
    sheet: &Sheet,
    measure: usize,
    staff: Option<usize>,
    voice: Option<usize>,
    beat: f64,
) -> Option<u64> {
    const NEAR: f64 = 1e-3;
    let grid = &sheet.grid;
    let (opens, closes) = grid.span(measure, measure);
    let (opens, closes) = (opens.to_f64(), closes.to_f64());
    let unit = grid.meter_at(measure).unit.max(1) as f64;
    let t = (opens + (beat - 1.0) / unit).clamp(opens, closes);
    let voices = &sheet.staves.get(staff.unwrap_or(0))?.voices;
    let mut last = None;
    for (vi, items) in voices.iter().enumerate() {
        if voice.is_some_and(|v| v != vi) {
            continue;
        }
        let mut inside = None;
        let mut onset = 0.0;
        for item in &items.items {
            let end = onset + item.dur().to_f64();
            if onset - NEAR <= t && t < end - NEAR {
                return Some(item.id());
            }
            if onset >= opens - NEAR && onset < closes - NEAR {
                inside = Some(item.id());
            }
            onset = end;
        }
        last = last.or(inside);
    }
    last
}

/// Ties written as `<tie startid endid>` rather than as `@tie`.
///
/// Verovio **normalizes one into the other**: a document this layer wrote with
/// `@tie="i"`/`"t"` comes back with those attributes gone and a `<tie>` element
/// hanging off the measure instead. Reading only the attribute would therefore
/// lose every tie the moment a score had been through the engraver once, which
/// is the ordinary case rather than an exotic one.
///
/// **A tie between two parts of one item is the emitter's**, written where
/// the item's value crosses a barline and never stored: read as the item's
/// own it would tie the item into whatever follows it, a tie nobody wrote.
/// And a tie the document draws to something that is not the next item of
/// the voice has no place in the model, so what is left is settled by the
/// rule every edit keeps ([`super::edit::settle_ties`]).
fn apply_ties(sheet: &mut Sheet, ties: &[(String, String)]) {
    let item = |reference: &str| -> Option<u64> {
        reference.strip_prefix('n')?.split('-').next()?.parse().ok()
    };
    let starts: Vec<u64> = ties
        .iter()
        .filter_map(|(start, end)| {
            let start = item(start)?;
            (item(end) != Some(start)).then_some(start)
        })
        .collect();
    for voice in sheet.voices_mut() {
        for item in &mut voice.items {
            if starts.contains(&item.id())
                && let Item::Note { tie, .. } = item
            {
                *tie = true;
            }
        }
    }
    super::edit::settle_ties(sheet);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::{Op, Slot, apply};
    use crate::notation::{
        add_spanner, concat, set_marks, sheet_to_mei, stack, tie, transpose_pitch, voice_to_sheet,
    };

    fn quarters(n: usize) -> Sheet {
        let voice: Vec<Slot> = (0..n).map(|_| Slot::note(vec![60], 8)).collect();
        voice_to_sheet(&voice, "4/4", "G2", "C")
    }

    /// The reader's one real obligation: what was written comes back.
    fn round_trips(sheet: &Sheet) -> Result<(), String> {
        let once = sheet_to_mei(sheet)?;
        let back = mei_to_sheet(&once)?;
        let twice = sheet_to_mei(&back)?;
        if once != twice {
            return Err(format!("wrote\n{once}\nread back and wrote\n{twice}"));
        }
        Ok(())
    }

    /// **Everything the model grew comes back**: the marks a note carries,
    /// what is written at a point, the lines between notes, the grid's
    /// changes, endings, signs and repeats, the staves' own facts and their
    /// groups -- written, read and written again to the same bytes.
    #[test]
    fn every_element_the_model_holds_comes_back() {
        use crate::notation::model::{Control, Group};
        let mut sheet = stack(quarters(16), &quarters(16), true).unwrap();
        let ids = |sheet: &Sheet, staff: usize| -> Vec<u64> {
            sheet.staves[staff].voices[0]
                .items
                .iter()
                .map(Item::id)
                .collect()
        };
        let top = ids(&sheet, 0);
        let low = ids(&sheet, 1);
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
                lyrics: vec!["Hal-".into(), "".into(), "one".into()],
                ..Marks::default()
            },
        );
        mark(
            &mut sheet,
            top[1],
            Marks {
                arpeggio: Some("down".into()),
                breath: Some("caesura".into()),
                ring: true,
                ornament: Some("ornamentHaydn".into()),
                ..Marks::default()
            },
        );
        // a beat repeat holds what it repeats
        let held = sheet.staves[0].voices[0].items[1].clone();
        if let Item::Note { pitches, marks, .. } = &mut sheet.staves[0].voices[0].items[2]
            && let Item::Note { pitches: p, .. } = &held
        {
            *pitches = p.clone();
            marks.beat_repeat = true;
        }
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
            ("diminuendo", top[12], top[14]),
        ] {
            sheet = add_spanner(sheet, kind, from, to).unwrap();
        }
        sheet.grid.keys = vec![(2, "D".into())];
        sheet.grid.meters.push(crate::notation::Meter {
            measure: 3,
            count: 3,
            unit: 4,
        });
        sheet.grid.endings = vec![(1, 1, "1".into()), (2, 2, "2".into())];
        sheet.grid.marks = vec![(0, "segno".into()), (3, "dalsegno".into())];
        sheet.staves[1].lines = Some(1);
        sheet.staves[0].label = "Flute".into();
        sheet.staves[0].abbr = "Fl.".into();
        sheet.staves[1].transpose = -2;
        sheet.staves[1].clefs = vec![(Ratio::from(2), "C3".into())];
        sheet.groups = vec![Group {
            first: 0,
            last: 1,
            symbol: "bracket".into(),
        }];
        round_trips(&sheet).unwrap();
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        let first = back.staves[0].voices[0].items[0].marks().unwrap().clone();
        assert_eq!(first.tremolo, Some(2));
        assert_eq!(first.fingering.as_deref(), Some("3"));
        assert_eq!(first.harmony.as_deref(), Some("Cm7"));
        assert_eq!(first.lyrics, vec!["Hal-", "", "one"]);
        let second = back.staves[0].voices[0].items[1].marks().unwrap().clone();
        assert_eq!(second.arpeggio.as_deref(), Some("down"));
        assert_eq!(second.breath.as_deref(), Some("caesura"));
        assert!(second.ring);
        assert_eq!(second.ornament.as_deref(), Some("ornamentHaydn"));
        assert!(
            back.staves[0].voices[0].items[2]
                .marks()
                .unwrap()
                .beat_repeat
        );
        assert_eq!(back.controls, sheet.controls);
        let mut kinds: Vec<&str> = back.spanners.iter().map(|s| s.kind.as_str()).collect();
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            vec![
                "8va",
                "bracket",
                "diminuendo",
                "ftrem",
                "gliss",
                "pedal",
                "phrase"
            ]
        );
        assert_eq!(back.grid.keys, sheet.grid.keys);
        assert_eq!(back.grid.meters, sheet.grid.meters);
        assert_eq!(back.grid.endings, sheet.grid.endings);
        assert_eq!(back.grid.marks, sheet.grid.marks);
        assert_eq!(back.staves[1].lines, Some(1));
        assert_eq!(
            (back.staves[0].label.as_str(), back.staves[0].abbr.as_str()),
            ("Flute", "Fl.")
        );
        assert_eq!(back.staves[1].transpose, -2);
        assert_eq!(back.staves[1].clefs, sheet.staves[1].clefs);
        assert_eq!(back.groups, sheet.groups);
        // and the two-note tremolo's notes keep their own values
        assert_eq!(back.len(), sheet.len());
    }

    /// What is written at an item of a measure drawn as a repeat is written
    /// at its beat of that measure -- the sign stands where the item's element
    /// would be -- and comes back on the item the measure holds there.
    #[test]
    fn what_is_written_into_a_measure_drawn_as_a_repeat_comes_back() {
        use crate::notation::model::Control;
        let mut sheet = quarters(12);
        sheet.grid.repeats = vec![1];
        // into the repeated measure, out of it, and a mark at a point of it
        sheet = add_spanner(sheet, "slur", 2, 6).unwrap();
        sheet = add_spanner(sheet, "crescendo", 7, 10).unwrap();
        sheet = add_spanner(sheet, "pedal", 5, 9).unwrap();
        sheet.controls.push(Control {
            kind: "reh".into(),
            on: 5,
            text: "B".into(),
            bpm: None,
        });
        let mei = sheet_to_mei(&sheet).unwrap();
        assert!(mei.contains("<mRpt/>"), "{mei}");
        let slur = "<slur staff=\"1\" startid=\"#n2\" tstamp2=\"1m+2\"/>";
        assert!(mei.contains(slur), "{mei}");
        let hairpin =
            "<hairpin form=\"cres\" staff=\"1\" tstamp=\"3\" layer=\"1\" endid=\"#n10\"/>";
        assert!(mei.contains(hairpin), "{mei}");
        assert!(mei.contains("<reh staff=\"1\" tstamp=\"1\" place=\"above\">"));
        assert!(mei.contains("<pedal staff=\"1\" tstamp=\"1\" layer=\"1\" dir=\"down\"/>"));

        // the repeated measure's items are the first's again, as new items
        let back = mei_to_sheet(&mei).unwrap();
        let ids: Vec<u64> = back.staves[0].voices[0]
            .items
            .iter()
            .map(Item::id)
            .collect();
        let repeated = &ids[4..8];
        let ends = |kind: &str| -> (u64, u64) {
            let found = back.spanners.iter().find(|s| s.kind == kind);
            found
                .map(|s| (s.from, s.to))
                .unwrap_or_else(|| panic!("no {kind} in {:?}", back.spanners))
        };
        assert_eq!(ends("slur"), (2, repeated[1]));
        assert_eq!(ends("crescendo"), (repeated[2], 10));
        assert_eq!(ends("pedal"), (repeated[0], 9));
        assert_eq!(back.controls.len(), 1);
        assert_eq!(back.controls[0].on, repeated[0]);
        // and it is written again as it was read
        round_trips(&back).unwrap();
    }

    /// A measure written as a repeat holds what the one before held, and a run
    /// of empty measures condensed into one numbered rest is as long as they.
    #[test]
    fn a_repeated_measure_and_a_numbered_rest_come_back() {
        let mut sheet = concat(quarters(8), &quarters(0)).unwrap();
        // two bars written, then two empty ones and two more written
        sheet.staves[0].voices[0].items.push(Item::Rest {
            id: 0,
            dur: Ratio::from(2),
        });
        sheet = concat(sheet, &quarters(4)).unwrap();
        sheet.grid.repeats = vec![1];
        sheet.grid.multirests = true;
        let mei = sheet_to_mei(&sheet).unwrap();
        assert!(mei.contains("<mRpt/>"), "{mei}");
        assert!(mei.contains("<multiRest num=\"2\"/>"), "{mei}");
        let back = mei_to_sheet(&mei).unwrap();
        assert_eq!(back.len(), sheet.len());
        assert_eq!(back.grid.repeats, vec![1]);
        assert!(back.grid.multirests);
        round_trips(&sheet).unwrap();
    }

    #[test]
    fn a_plain_score_written_read_and_written_again_is_the_same_bytes() {
        round_trips(&quarters(6)).unwrap();
    }

    #[test]
    fn the_notes_come_back_with_their_values_and_spelling() {
        let mut sheet = quarters(2);
        for voice in sheet.voices_mut() {
            for item in &mut voice.items {
                if let Item::Note { pitches, .. } = item {
                    pitches[0] = transpose_pitch(&pitches[0], 3, 6);
                }
            }
        }
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        let items = &back.staves[0].voices[0].items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].dur(), Ratio::new(1, 4));
        // an F sharp comes back an F sharp, not a G flat: the spelling is on
        // the page and the reader takes it from there
        assert_eq!(items[0].pitches()[0].step, Step::F);
        assert_eq!(items[0].pitches()[0].alter, 1);
    }

    #[test]
    fn a_note_split_across_a_barline_comes_back_as_the_one_note_it_was() {
        // Five quarters in 4/4 and then a note that overruns the bar: the
        // emitter splits and ties it, and reading it back has to undo that or
        // the model grows an item every trip.
        let sheet = apply(
            quarters(3),
            &Op::Insert {
                after: None,
                dur: Ratio::new(1, 2),
                pitches: Vec::new(),
                position: None,
                staff: 0,
                voice: 0,
            },
        )
        .unwrap();
        let mei = sheet_to_mei(&sheet).unwrap();
        let back = mei_to_sheet(&mei).unwrap();
        assert_eq!(
            back.staves[0].voices[0].items.len(),
            sheet.staves[0].voices[0].items.len(),
            "a split part rejoined into the item it came from"
        );
        round_trips(&sheet).unwrap();
    }

    /// The engraver hands a tie back as an element between two ids. One
    /// between two parts of the same item is the barline's, written by the
    /// emitter and never stored; one between two items is the item's.
    #[test]
    fn a_tie_between_two_parts_of_one_item_is_not_the_items() {
        let doc = |ties: &str| {
            format!(
                "<mei xmlns=\"http://www.music-encoding.org/ns/mei\"><music><body><mdiv><score>\
                 <scoreDef meter.count=\"2\" meter.unit=\"4\"><staffGrp>\
                 <staffDef n=\"1\" lines=\"5\" clef.shape=\"G\" clef.line=\"2\"/></staffGrp></scoreDef>\
                 <section><measure n=\"1\"><staff n=\"1\"><layer n=\"1\">\
                 <note xml:id=\"n1\" dur=\"4\" pname=\"c\" oct=\"4\"/>\
                 <note xml:id=\"n2\" dur=\"4\" pname=\"e\" oct=\"4\"/>\
                 </layer></staff></measure><measure n=\"2\"><staff n=\"1\"><layer n=\"1\">\
                 <note xml:id=\"n2-2\" dur=\"4\" pname=\"e\" oct=\"4\"/>\
                 <note xml:id=\"n3\" dur=\"4\" pname=\"{}\" oct=\"4\"/>\
                 </layer></staff>{ties}</measure></section></score></mdiv></body></music></mei>",
                if ties.contains("#n3") { "e" } else { "g" }
            )
        };
        let tied = |mei: &str| -> Vec<u64> {
            let sheet = mei_to_sheet(mei).unwrap();
            let items = &sheet.staves[0].voices[0].items;
            assert_eq!(items.len(), 3, "the split note is one item");
            items
                .iter()
                .filter(|i| matches!(i, Item::Note { tie: true, .. }))
                .map(Item::id)
                .collect()
        };
        // the barline's alone: the half note ties into nothing after it
        let inner = "<tie startid=\"#n2\" endid=\"#n2-2\"/>";
        assert_eq!(tied(&doc(inner)), Vec::<u64>::new());
        // and with the item's own, into the note after its last part
        let both = format!("{inner}<tie startid=\"#n2-2\" endid=\"#n3\"/>");
        assert_eq!(tied(&doc(&both)), [2]);
    }

    #[test]
    fn several_staves_and_voices_come_back_where_they_were() {
        let duo = stack(quarters(4), &quarters(4), true).unwrap();
        let back = mei_to_sheet(&sheet_to_mei(&duo).unwrap()).unwrap();
        assert_eq!(back.staves.len(), 2);
        assert_eq!(back.staves[1].clef, duo.staves[1].clef);
        round_trips(&duo).unwrap();

        let voices = stack(quarters(4), &quarters(4), false).unwrap();
        let back = mei_to_sheet(&sheet_to_mei(&voices).unwrap()).unwrap();
        assert_eq!(back.staves[0].voices.len(), 2);
    }

    #[test]
    fn the_marks_and_the_spanners_survive() {
        let mut sheet = quarters(4);
        let ids: Vec<u64> = sheet.staves[0].voices[0]
            .items
            .iter()
            .map(Item::id)
            .collect();
        sheet = set_marks(
            sheet,
            ids[0],
            Marks {
                articulations: vec!["stacc".into()],
                dynamic: Some("mf".into()),
                stem: Some("up".into()),
                ..Marks::default()
            },
        )
        .unwrap();
        sheet = add_spanner(sheet, "slur", ids[0], ids[2]).unwrap();
        sheet = add_spanner(sheet, "crescendo", ids[1], ids[3]).unwrap();
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        let first = &back.staves[0].voices[0].items[0];
        let marks = first.marks().unwrap();
        assert_eq!(marks.articulations, vec!["stacc".to_string()]);
        assert_eq!(marks.dynamic.as_deref(), Some("mf"));
        assert_eq!(marks.stem.as_deref(), Some("up"));
        assert_eq!(back.spanners.len(), 2);
    }

    #[test]
    fn a_tie_survives_the_engravers_own_spelling_of_it() {
        // We write `@tie="i"/"t"`; verovio hands back `<tie startid endid/>`.
        // Reading only our own spelling would lose every tie a score picked up
        // by passing through the engraver once.
        let mei = "<mei><music><body><mdiv><score>\
            <scoreDef meter.count=\"4\" meter.unit=\"4\" keysig=\"0\">\
            <staffGrp><staffDef n=\"1\" clef.shape=\"G\" clef.line=\"2\"/></staffGrp>\
            </scoreDef><section><measure n=\"1\">\
            <staff n=\"1\"><layer n=\"1\">\
            <note xml:id=\"n1\" dur=\"4\" oct=\"4\" pname=\"c\"/>\
            <note xml:id=\"n2\" dur=\"4\" oct=\"4\" pname=\"c\"/>\
            </layer></staff><tie startid=\"#n1\" endid=\"#n2\"/>\
            </measure></section></score></mdiv></body></music></mei>";
        let sheet = mei_to_sheet(mei).unwrap();
        assert!(
            matches!(
                sheet.staves[0].voices[0].items[0],
                Item::Note { tie: true, .. }
            ),
            "{:?}",
            sheet.staves[0].voices[0].items[0]
        );
    }

    #[test]
    fn the_header_the_barlines_and_the_breaks_come_back() {
        let mut sheet = concat(quarters(4), &quarters(4)).unwrap();
        sheet.header = Header {
            title: "Six bars".into(),
            composer: "A. Composer".into(),
            ..Header::default()
        };
        sheet.grid.barlines = vec![(0, "rptend".to_string())];
        sheet.grid.breaks = vec![(1, "system".to_string())];
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        assert_eq!(back.header.title, "Six bars");
        assert_eq!(back.header.composer, "A. Composer");
        assert_eq!(back.grid.barlines, vec![(0, "rptend".to_string())]);
        assert_eq!(back.grid.breaks, vec![(1, "system".to_string())]);
        round_trips(&sheet).unwrap();
    }

    #[test]
    fn a_title_carrying_an_ampersand_does_not_end_the_document_early() {
        let mut sheet = quarters(2);
        sheet.header.title = "Bell & Drum <2>".into();
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        assert_eq!(back.header.title, "Bell & Drum <2>");
    }

    #[test]
    fn a_tuplet_comes_back_as_the_exact_thirds_it_was() {
        let mut sheet = quarters(3);
        for voice in sheet.voices_mut() {
            for item in &mut voice.items {
                *item = item.with_dur(Ratio::new(1, 12));
            }
        }
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        assert_eq!(back.staves[0].voices[0].items[0].dur(), Ratio::new(1, 12));
    }

    #[test]
    fn a_measure_rest_comes_back_as_long_as_its_measure() {
        let duo = stack(quarters(8), &quarters(4), true).unwrap();
        let mei = sheet_to_mei(&duo).unwrap();
        assert!(mei.contains("<mRest/>"), "the short staff is padded: {mei}");
        let back = mei_to_sheet(&mei).unwrap();
        // The padding is the emitter's and does not come back: the lower staff
        // was written with four quarters and has four quarters. A score that
        // gained a bar of silence for having been saved would be the defect.
        let lower = &back.staves[1].voices[0];
        let written: Ratio = lower.items.iter().fold(Ratio::ZERO, |a, i| a + i.dur());
        assert_eq!(written, Ratio::ONE, "four quarters: {lower:?}");
        round_trips(&duo).unwrap();
    }

    #[test]
    fn a_beam_somebody_chose_is_written_and_read_back() {
        let mut sheet = quarters(4);
        for voice in sheet.voices_mut() {
            for item in &mut voice.items {
                *item = item.with_dur(Ratio::new(1, 8));
            }
        }
        let ids: Vec<u64> = sheet.staves[0].voices[0]
            .items
            .iter()
            .map(Item::id)
            .collect();
        let sheet = add_spanner(sheet, "beam", ids[0], ids[3]).unwrap();
        let mei = sheet_to_mei(&sheet).unwrap();
        assert!(mei.contains("<beam>") && mei.contains("</beam>"), "{mei}");
        round_trips(&sheet).unwrap();
    }

    #[test]
    fn a_document_from_somewhere_else_is_read_and_given_ids_of_our_own() {
        let mei = "<mei><music><body><mdiv><score>\
            <scoreDef meter.count=\"3\" meter.unit=\"4\" keysig=\"1s\">\
            <staffGrp><staffDef n=\"1\"><clef shape=\"F\" line=\"4\"/></staffDef></staffGrp>\
            </scoreDef><section><measure n=\"1\" right=\"end\">\
            <staff n=\"1\"><layer n=\"1\">\
            <note xml:id=\"m1ocu09p\" dur=\"4\" oct=\"3\" pname=\"g\"/>\
            <note xml:id=\"o11ivu7y\" dur=\"8\" dots=\"1\" oct=\"3\" pname=\"a\" accid.ges=\"s\"/>\
            <rest xml:id=\"w1pe5o6r\" dur=\"8\"/>\
            </layer></staff></measure></section></score></mdiv></body></music></mei>";
        let sheet = mei_to_sheet(mei).unwrap();
        assert_eq!(sheet.key, "G");
        assert_eq!(sheet.staves[0].clef, "F4");
        assert_eq!(sheet.grid.meter_at(0).count, 3);
        let items = &sheet.staves[0].voices[0].items;
        assert_eq!(items[1].dur(), Ratio::new(3, 16), "a dotted eighth");
        assert_eq!(items[1].pitches()[0].alter, 1);
        assert!(!items[1].pitches()[0].forced, "sounding, so not printed");
        // ids are minted here rather than trusted from a document that minted
        // them for its own purposes
        assert!(items.iter().all(|i| i.id() != 0));
        // and the barline the last measure gets by default is not an override
        assert!(sheet.grid.barlines.is_empty());
    }

    #[test]
    fn a_note_with_no_accidental_takes_what_the_armature_says() {
        // The emitter writes nothing where the armature already says it, which
        // is correct engraving -- so a reader that did not apply the armature
        // would turn every B flat in E flat into a B natural, silently, on the
        // first save. The same mistake the encoder once made in the other
        // direction, caught by the same rule read backwards.
        let sheet = voice_to_sheet(&[Slot::note(vec![70], 8)], "4/4", "G2", "Eb");
        let mei = sheet_to_mei(&sheet).unwrap();
        // Nothing is *printed*: the armature says it. The sounding alteration
        // is still stated, as a child `<accid accid.ges>`, which is one of the
        // four places an accidental can be and the one our own emitter uses.
        assert!(
            !mei.contains("<accid accid=\""),
            "nothing is printed: {mei}"
        );
        assert!(mei.contains("accid.ges=\"f\""), "{mei}");
        let back = mei_to_sheet(&mei).unwrap();
        let pitch = back.staves[0].voices[0].items[0].pitches()[0];
        assert_eq!(pitch.step, Step::B);
        assert_eq!(pitch.alter, -1, "the armature is what says so");
        round_trips(&sheet).unwrap();
    }

    #[test]
    fn an_accidental_holds_for_its_measure_and_a_new_one_starts_again() {
        // c#4 then c4 in one bar: the second is written natural, and the third,
        // in the next bar, is a plain c that the armature leaves alone.
        let mei = "<mei><music><body><mdiv><score>\
            <scoreDef meter.count=\"1\" meter.unit=\"4\" keysig=\"0\">\
            <staffGrp><staffDef n=\"1\" clef.shape=\"G\" clef.line=\"2\"/></staffGrp>\
            </scoreDef><section>\
            <measure n=\"1\"><staff n=\"1\"><layer n=\"1\">\
            <note dur=\"8\" oct=\"4\" pname=\"c\" accid=\"s\"/>\
            <note dur=\"8\" oct=\"4\" pname=\"c\"/>\
            </layer></staff></measure>\
            <measure n=\"2\"><staff n=\"1\"><layer n=\"1\">\
            <note dur=\"4\" oct=\"4\" pname=\"c\"/>\
            </layer></staff></measure>\
            </section></score></mdiv></body></music></mei>";
        let sheet = mei_to_sheet(mei).unwrap();
        let items = &sheet.staves[0].voices[0].items;
        assert_eq!(items[0].pitches()[0].alter, 1, "the sharp as written");
        assert_eq!(items[1].pitches()[0].alter, 1, "still sharp: same bar");
        assert_eq!(items[2].pitches()[0].alter, 0, "a new bar starts again");
    }

    #[test]
    fn what_is_not_a_score_says_so_rather_than_reading_as_an_empty_one() {
        assert!(mei_to_sheet("<not xml").is_err());
        let err = mei_to_sheet("<mei><music/></mei>").unwrap_err();
        assert!(err.contains("<score>"), "{err}");
    }

    #[test]
    fn a_tie_the_caller_wrote_is_kept_apart_from_the_barlines_own() {
        let sheet = tie(quarters(4), 1, true).unwrap();
        let back = mei_to_sheet(&sheet_to_mei(&sheet).unwrap()).unwrap();
        assert!(matches!(
            back.staves[0].voices[0].items[0],
            Item::Note { tie: true, .. }
        ));
        assert_eq!(back.staves[0].voices[0].items.len(), 4, "not merged");
    }
}
