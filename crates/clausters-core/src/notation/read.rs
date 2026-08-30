//! The reader: a document back into the model.
//!
//! The other return path, and not the one the interpreter is. [`super::perform`]
//! turns a model into sound; this turns a *document* into a model — which is
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
//! document is musical — the header, the barlines, the breaks and the beams a
//! writer chose — so what is left outside it is what the engraver recomputes
//! when nobody chose: automatic beaming, the line breaks that merely fit, the
//! staff geometry. Those are not read and are not loss. Anything else a
//! document carries and this cannot represent is a **gap to write down**, and
//! the plan says so rather than the reader swallowing it.
//!
//! **Ids are how a page names a note**, so they survive: an element written by
//! this layer carries the model's own id (`n7`, and `n7-2` for a piece of one
//! split across a barline), and reading it back recovers the item — the split
//! pieces rejoin into the one item they came from, which is what makes a sheet
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
/// When the document is not readable XML, or carries no `<score>` — the two
/// cases where there is nothing to read rather than something to skip.
pub fn mei_to_sheet(mei: &str) -> Result<Sheet, String> {
    let doc = Document::parse(mei).map_err(|e| format!("the document is not readable XML: {e}"))?;
    let score = find(doc.root_element(), "score")
        .ok_or_else(|| "the document carries no <score> to read".to_string())?;

    let mut sheet = Sheet {
        header: read_header(doc.root_element()),
        ..Sheet::default()
    };
    let mut clefs: Vec<String> = Vec::new();
    let mut meters: Vec<Meter> = Vec::new();
    // [staff][voice] -> the items read so far, appended measure by measure.
    let mut content: Vec<Vec<Vec<Item>>> = Vec::new();
    let mut beams: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut ties: Vec<(String, String)> = Vec::new();
    let mut attached: Vec<(String, String, Option<String>)> = Vec::new();
    let mut pending_break: Option<String> = None;
    let mut measure = 0usize;

    for child in score.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "scoreDef" => {
                if clefs.is_empty() {
                    clefs = read_clefs(child);
                    sheet.key = read_key(child);
                }
                if let Some(meter) = read_meter(child, measure) {
                    meters.push(meter);
                }
            }
            "section" => {
                for node in child.children().filter(Node::is_element) {
                    match node.tag_name().name() {
                        "pb" => pending_break = Some("page".to_string()),
                        "sb" => pending_break = Some("system".to_string()),
                        "scoreDef" => {
                            if let Some(meter) = read_meter(node, measure) {
                                meters.push(meter);
                            }
                        }
                        "measure" => {
                            if let Some(kind) = pending_break.take() {
                                sheet.grid.breaks.push((measure, kind));
                            }
                            if let Some(right) = node.attribute("right")
                                && right != "single"
                            {
                                sheet.grid.barlines.push((measure, right.to_string()));
                            }
                            let key = sheet.key.clone();
                            read_measure(
                                node,
                                &key,
                                &mut content,
                                &mut beams,
                                &mut ties,
                                &mut attached,
                            );
                            measure += 1;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    sheet.grid.meters = if meters.is_empty() {
        Grid::default().meters
    } else {
        meters
    };
    // The last measure's barline is `end` by default and the emitter writes it
    // unasked, so keeping it would make every read-back carry an override
    // nobody wrote.
    if let Some(last) = measure.checked_sub(1) {
        sheet
            .grid
            .barlines
            .retain(|(m, kind)| !(*m == last && kind == "end"));
    }

    sheet.staves = content
        .into_iter()
        .enumerate()
        .map(|(si, voices)| Staff {
            clef: clefs.get(si).cloned().unwrap_or_else(|| "G2".to_string()),
            voices: voices.into_iter().map(|items| Voice { items }).collect(),
        })
        .collect();
    if sheet.staves.is_empty() {
        sheet.staves = vec![Staff::default()];
    }

    // The order matters, and each step is where it is for a reason. The
    // emitter's padding goes first, while "it has no id" still identifies it
    // and before anything has renumbered; it is always trailing, so dropping it
    // moves no position anything else holds. Then ids, so a beam read as a run
    // of *positions* can name its ends before rejoining the split pieces moves
    // them.
    drop_padding(&mut sheet);
    sheet.assign_ids();
    apply_beams(&mut sheet, &beams);
    rejoin(&mut sheet);
    apply_attachments(&mut sheet, &attached);
    apply_ties(&mut sheet, &ties);
    Ok(sheet)
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
/// Verovio's importers put the title in two different places — `<titleStmt>`,
/// which is where this layer writes it, and `<workList>`, which is where the
/// ABC importer puts it — so both are read and the first non-empty one wins.
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
    }
}

/// The staves' clefs, in order, as the model spells one (`"G2"`).
fn read_clefs(score_def: Node) -> Vec<String> {
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
            format!("{shape}{line}")
        })
        .collect()
}

/// The key, as the tonic name the model holds.
///
/// Written `key.sig` by this layer and normalized to `keysig` by verovio, so
/// both spellings are read — which is the sort of difference that is invisible
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

/// One measure: its layers appended to the content, and what hangs off it
/// collected for later.
fn read_measure(
    measure: Node,
    key: &str,
    content: &mut Vec<Vec<Vec<Item>>>,
    beams: &mut Vec<(usize, usize, usize, usize)>,
    ties: &mut Vec<(String, String)>,
    attached: &mut Vec<(String, String, Option<String>)>,
) {
    for staff in measure.children().filter(|n| n.has_tag_name("staff")) {
        // An accidental holds for the rest of its measure, at its own step and
        // octave, on this staff. A new measure starts again from the armature —
        // the ordinary convention, and the one the emitter writes with, so the
        // two have to agree or a score means something different after a save.
        let mut in_force: HashMap<(i32, i32), i32> = HashMap::new();
        let si = number(staff, 1) - 1;
        while content.len() <= si {
            content.push(Vec::new());
        }
        for layer in staff.children().filter(|n| n.has_tag_name("layer")) {
            let vi = number(layer, 1) - 1;
            while content[si].len() <= vi {
                content[si].push(Vec::new());
            }
            let mut here = Vec::new();
            read_items(
                layer,
                Ratio::ONE,
                key,
                &mut in_force,
                &mut content[si][vi],
                &mut here,
            );
            beams.extend(here.into_iter().map(|(a, b)| (si, vi, a, b)));
        }
    }
    for node in measure.children().filter(Node::is_element) {
        let name = node.tag_name().name();
        let start = node.attribute("startid").map(strip_hash);
        let end = node.attribute("endid").map(strip_hash);
        match (name, start) {
            ("tie", Some(start)) => {
                if let Some(end) = end {
                    ties.push((start, end));
                }
            }
            ("slur" | "hairpin", Some(start)) => attached.push((name.to_string(), start, end)),
            ("dynam", Some(start)) => {
                let text = node.text().unwrap_or_default().trim().to_string();
                attached.push(("dynam".to_string(), start, Some(text)));
            }
            ("trill" | "mordent" | "turn" | "fermata", Some(start)) => {
                attached.push(("ornament".to_string(), start, Some(name.to_string())))
            }
            _ => {}
        }
    }
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
    beams: &mut Vec<(usize, usize)>,
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
                read_items(child, scale, key, in_force, out, beams);
                if out.len() > first {
                    beams.push((first, out.len() - 1));
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
                    beams,
                );
            }
            "chord" => {
                let dur = duration(child, scale).unwrap_or(Ratio::new(1, 4));
                let pitches: Vec<Pitch> = child
                    .children()
                    .filter(|n| n.has_tag_name("note"))
                    .filter_map(|n| pitch_of(n, key, in_force))
                    .collect();
                out.push(Item::Note {
                    id: id_of(child),
                    pitches,
                    dur,
                    tie: false,
                    marks: marks_of(child),
                });
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
/// which is exactly what `forced` holds — so the distinction the emitter makes
/// survives the trip back.
///
/// **A note with no accidental of its own is not a natural.** It takes what is
/// in force: an accidental printed earlier in this measure at its step and
/// octave, or failing that the key signature's. The emitter writes nothing
/// where the armature already says it — which is correct engraving — so a
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

/// What one note carries on itself. What hangs off the measure instead — a
/// dynamic, an ornament — is added later, by `apply_attachments`.
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
        ..Marks::default()
    }
}

/// The model id this element was written from, or `0` when it was written
/// somewhere else — `Sheet::assign_ids` mints those.
fn id_of(node: Node) -> u64 {
    let Some(id) = node.attribute(("http://www.w3.org/XML/1998/namespace", "id")) else {
        return 0;
    };
    let Some(rest) = id.strip_prefix('n') else {
        return 0;
    };
    // `n7` is the item; `n7-2` is a piece of it split across a barline.
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
/// would grow the score by a rest on every trip through a document — the score
/// would gain a bar of silence for having been saved.
///
/// They are known by having **no id**: every element this layer writes from an
/// item carries the item's own, and only what the emitter made up has none. So
/// the rule holds only for a document this layer wrote, which is what `ours`
/// tests — a document from anywhere else has ids of its own shape, none of them
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

/// Rejoin the pieces a barline split, and size the measure rests.
///
/// The emitter splits an item that overruns a barline and ties the halves, so a
/// document holds two elements where the model held one. Both carry the same
/// model id, which is what lets this put them back — and putting them back is
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

/// Put back what hung off the measures: the spanners, the dynamics, the
/// ornaments, and the beams read from the elements they wrapped.
fn apply_attachments(sheet: &mut Sheet, attached: &[(String, String, Option<String>)]) {
    let ids: BTreeMap<String, u64> = sheet
        .voices()
        .flat_map(|v| v.items.iter())
        .map(|i| (format!("n{}", i.id()), i.id()))
        .collect();
    let resolve = |reference: &str| -> Option<u64> { ids.get(reference).copied() };

    for (kind, start, extra) in attached {
        match kind.as_str() {
            "slur" | "hairpin" => {
                let (Some(from), Some(to)) = (resolve(start), extra.as_deref().and_then(&resolve))
                else {
                    continue;
                };
                sheet.spanners.push(Spanner {
                    kind: if kind == "slur" {
                        "slur".to_string()
                    } else {
                        "crescendo".to_string()
                    },
                    from,
                    to,
                });
            }
            "dynam" | "ornament" => {
                let (Some(id), Some(text)) = (resolve(start), extra.as_deref()) else {
                    continue;
                };
                for voice in sheet.voices_mut() {
                    for item in &mut voice.items {
                        if item.id() == id
                            && let Item::Note { marks, .. } = item
                        {
                            if kind == "dynam" {
                                marks.dynamic = Some(text.to_string());
                            } else {
                                marks.ornament = Some(text.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Ties written as `<tie startid endid>` rather than as `@tie`.
///
/// Verovio **normalizes one into the other**: a document this layer wrote with
/// `@tie="i"`/`"t"` comes back with those attributes gone and a `<tie>` element
/// hanging off the measure instead. Reading only the attribute would therefore
/// lose every tie the moment a score had been through the engraver once, which
/// is the ordinary case rather than an exotic one.
fn apply_ties(sheet: &mut Sheet, ties: &[(String, String)]) {
    let starts: Vec<u64> = ties
        .iter()
        .filter_map(|(start, _)| start.strip_prefix('n')?.split('-').next()?.parse().ok())
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
            "a split piece rejoined into the item it came from"
        );
        round_trips(&sheet).unwrap();
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
