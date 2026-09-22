//! **A source made of spans, as the thing an endpoint has to make.**
//!
//! A join owns no samples: it is spans of the takes the table already holds
//! ([`Location::Segments`]), and `clausters-document` says in so many words
//! that it does not build one -- *"this crate holds source ids rather than
//! sources: whoever has the samples fills it in when it realizes the join"*.
//! **This is the reading of that recipe**, so that whoever realizes it does not
//! read it again.
//!
//! It was read three times before this module: at **open** in the GUI host
//! (walking a session's table into `/buffer_stitch`), at **edit time** in the
//! Python client (the same walk over the source an intent carried), and nowhere
//! at all in the host's edit path -- which is why a join made in a standalone
//! host produced a box that drew empty, sounded through nothing and had no
//! length to stop an edge at. The two that existed had already drifted on the
//! part nobody looks at: the **channel map**.
//!
//! # What it does not do
//!
//! It opens nothing, allocates nothing and names no buffer of its own: the
//! join's *own* number is the caller's, like every other resource. What comes
//! back is what to make, in the terms the wire takes.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use clausters_document::SourceId;
use clausters_document::session::{Location, Part, Source};

/// **What a caller knows about a source that the document does not**: where its
/// samples are, how wide they are, and how long.
///
/// The document's own [`SourceInfo`](clausters_document::multitrack::nodes::SourceInfo)
/// answers the first two, which is all a *plan* needs; a join needs the third,
/// because a part that names no range contributes the whole of its source and
/// only the caller knows how much that is.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Held {
    /// The server buffer holding the samples.
    pub buffer: i32,
    /// How many channels it has.
    pub channels: usize,
    /// How many frames, for a part that takes all of it.
    pub frames: u64,
    /// **The rate its samples were written at**, or `0` where the caller does
    /// not know -- and then the part is read as though it were the join's own,
    /// which is what every part of a session recorded at one rate is.
    #[serde(default)]
    pub rate: f64,
}

/// One part of a join, resolved: a run of one buffer, and which of its channels
/// feeds each channel of the join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StitchPart {
    /// The buffer this span is read from.
    pub buffer: i32,
    /// Its first frame.
    pub start: u64,
    /// How many frames it contributes.
    pub frames: u64,
    /// Frames of linear fade at its head.
    pub fade_in: u64,
    /// Frames of linear fade at its tail.
    pub fade_out: u64,
    /// **One entry per channel of the join**: which channel of this part feeds
    /// it, or `-1` for silence.
    ///
    /// Always spelled in full, which is what makes the wire's group fixed
    /// width -- and what decides the case nobody writes a test for: a part
    /// **narrower than the join repeats**, so a mono take in a stereo join is
    /// heard on both sides rather than on one.
    pub channels: Vec<i32>,
}

/// A join, resolved: what to allocate and what to fill it with.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stitch {
    /// How wide the join is.
    pub channels: usize,
    /// Its rate, or `0.0` for the server's.
    pub rate: f64,
    /// How many frames it comes to, which is the sum of its parts.
    pub frames: u64,
    /// The parts, in the order they play.
    pub parts: Vec<StitchPart>,
}

/// **What a source made of segments comes to**, or `None`.
///
/// `None` where it is not a join at all, where it has no parts, or where a part
/// names a source the caller has not resolved. The last is the important one
/// and it is deliberate: a join is left **unmade rather than half made**, since
/// a box over a source nobody answered for draws empty and plays nothing, which
/// is what an unresolved source has always meant here -- and the round after the
/// missing take lands makes it.
pub fn stitch(source: &Source, held: &HashMap<SourceId, Held>) -> Option<Stitch> {
    let Location::Segments { parts } = &source.location else {
        return None;
    };
    if parts.is_empty() {
        return None;
    }
    let resolved: Option<Vec<Held>> = parts
        .iter()
        .map(|part| held.get(&part.source.source).copied())
        .collect();
    let resolved = resolved?;
    // **The join's width is stated or it is the widest part's.** Stated wins
    // because a source that says how wide it is has been written down that way;
    // otherwise a join of a mono and a stereo take is stereo, since narrowing
    // it would drop a channel nobody asked to lose.
    let channels = source
        .channels
        .map(|c| c.max(1) as usize)
        .unwrap_or_else(|| {
            resolved
                .iter()
                .map(|h| h.channels)
                .max()
                .unwrap_or(1)
                .max(1)
        });
    // **The join's rate is stated or it is its first part's.** A join is one
    // buffer with one rate, and a source that says which has been written down
    // that way; a minted one says nothing yet, and then it is the rate of what
    // it is made of, which for every join of one session's takes is that
    // session's.
    let rate = source
        .sample_rate
        .filter(|r| *r > 0.0)
        .or_else(|| resolved.iter().map(|h| h.rate).find(|r| *r > 0.0))
        .unwrap_or(0.0);
    let mut frames = 0u64;
    let mut out = Vec::with_capacity(parts.len());
    for (part, take) in parts.iter().zip(&resolved) {
        let (start, span) = span_of(part, take);
        // **A part states what it contributes to the join, not what it reads.**
        // The two are the same number only while its samples were written at
        // the join's rate; a 44.1 kHz span of 44100 frames is one second, and
        // one second of a 48 kHz join is 48000 of its frames. The server reads
        // the span back out of that by the same ratio, which it takes off the
        // buffers themselves -- so nothing about the ratio travels.
        let contributed = match (take.rate, rate) {
            (src, join) if src > 0.0 && join > 0.0 && src != join => {
                ((span as f64) * join / src).round() as u64
            }
            _ => span,
        };
        frames += contributed;
        out.push(StitchPart {
            buffer: take.buffer,
            start,
            frames: contributed,
            fade_in: part.fade_in,
            fade_out: part.fade_out,
            channels: map_of(part, take.channels, channels),
        });
    }
    Some(Stitch {
        channels,
        rate,
        frames,
        parts: out,
    })
}

/// [`stitch`] as the JSON both client doors carry.
///
/// `source` is a source-table entry as the document writes one -- a minted
/// source as an intent carries it reads the same, its `id` beside the rest --
/// and `held` is the caller's table: source id to `{"buffer", "channels",
/// "frames", "rate"}`. The answer is the [`Stitch`] with its parts' fades as
/// `fadeIn`/`fadeOut`, or `null` where there is nothing to make: not a join, no
/// parts, or a part over a source the caller has not resolved.
pub fn stitch_json(source: &str, held: &str) -> String {
    let Ok(source) = serde_json::from_str::<Source>(source) else {
        return "null".into();
    };
    let held: HashMap<SourceId, Held> = serde_json::from_str::<HashMap<String, Held>>(held)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, entry)| id.parse::<u64>().ok().map(|id| (SourceId(id), entry)))
        .collect();
    match stitch(&source, &held) {
        Some(made) => serde_json::to_value(made)
            .unwrap_or(Value::Null)
            .to_string(),
        None => "null".into(),
    }
}

/// The span a part reads **of its source**, in that source's own frames: its
/// range, or the whole of it.
fn span_of(part: &Part, take: &Held) -> (u64, u64) {
    match &part.source.range {
        Some(range) => (range.start, range.len()),
        None => (0, take.frames),
    }
}

/// Which channel of a part feeds each channel of the join.
///
/// The part's own map wins where it has one; otherwise a part **repeats** to
/// fill the join, which is the rule that makes a mono take in a stereo join
/// heard on both sides. A channel the map does not name is silence (`-1`), as
/// on the wire.
fn map_of(part: &Part, width: usize, channels: usize) -> Vec<i32> {
    let width = width.max(1);
    (0..channels)
        .map(|channel| match &part.channels {
            Some(map) => map.get(channel).copied().unwrap_or(-1),
            None => (channel % width) as i32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::{Lifetime, Range, SourceRef};

    fn part(source: u64, range: Option<(u64, u64)>) -> Part {
        Part {
            source: SourceRef {
                source: SourceId(source),
                lifetime: Lifetime::Session,
                generation: 0,
                range: range.map(|(start, end)| Range { start, end }),
            },
            fade_in: 0,
            fade_out: 0,
            channels: None,
        }
    }

    fn join(parts: Vec<Part>, channels: Option<u32>) -> Source {
        let mut source = Source::volatile(Lifetime::Session);
        source.location = Location::Segments { parts };
        source.channels = channels;
        source.sample_rate = Some(48_000.0);
        source
    }

    fn held(entries: &[(u64, i32, usize, u64)]) -> HashMap<SourceId, Held> {
        entries
            .iter()
            .map(|(id, buffer, channels, frames)| {
                (
                    SourceId(*id),
                    Held {
                        buffer: *buffer,
                        channels: *channels,
                        frames: *frames,
                        // Unknown, which is what a table that says nothing
                        // about rates means: read at the join's own.
                        rate: 0.0,
                    },
                )
            })
            .collect()
    }

    /// **The JSON door is the same reading**: a minted source as an intent
    /// carries it, `id` and all, and the caller's table keyed by string.
    #[test]
    fn the_json_door_reads_a_minted_source_and_answers_null_for_nothing() {
        // A minted source as an intent carries it: the table entry, with its id
        // flattened beside the rest.
        let mut part = part(1, Some((10, 20)));
        part.fade_in = 3;
        let mut minted = serde_json::to_value(join(vec![part], None)).unwrap();
        minted["id"] = serde_json::json!(5);
        let held = r#"{"1": {"buffer": 7, "channels": 1, "frames": 100}}"#;
        let answer: Value = serde_json::from_str(&stitch_json(&minted.to_string(), held)).unwrap();
        assert_eq!(answer["frames"], 10);
        assert_eq!(answer["parts"][0]["buffer"], 7);
        assert_eq!(answer["parts"][0]["fadeIn"], 3);
        assert_eq!(
            stitch_json(&minted.to_string(), "{}"),
            "null",
            "a part nobody loaded"
        );
    }

    /// **A join takes parts at other rates, and each says what it contributes.**
    /// A join owns no samples, so nothing is converted: a part states how much
    /// of the join it fills, and the server reads its source back out of that
    /// by the ratio the two rates make -- which it takes off the buffers, so no
    /// ratio travels. One second of a 44.1 kHz take is 48000 frames of a 48 kHz
    /// join, and the join is as long as what its parts fill.
    #[test]
    fn a_part_at_another_rate_states_what_it_fills_of_the_join() {
        let source = join(
            vec![part(1, Some((0, 44_100))), part(2, Some((0, 24_000)))],
            Some(1),
        );
        let table = HashMap::from([
            (
                SourceId(1),
                Held {
                    buffer: 7,
                    channels: 1,
                    frames: 44_100,
                    rate: 44_100.0,
                },
            ),
            (
                SourceId(2),
                Held {
                    buffer: 8,
                    channels: 1,
                    frames: 48_000,
                    rate: 48_000.0,
                },
            ),
        ]);
        let made = stitch(&source, &table).expect("both parts are there");
        assert_eq!(
            made.rate, 48_000.0,
            "the join's own, as the source states it"
        );
        assert_eq!(
            made.parts[0].frames, 48_000,
            "a second of the 44.1 kHz take fills a second of the join"
        );
        assert_eq!(made.parts[0].start, 0, "read from its own first frame");
        assert_eq!(
            made.parts[1].frames, 24_000,
            "and a part at the join's rate fills what it reads"
        );
        assert_eq!(made.frames, 72_000, "the join is what its parts fill");
    }

    /// A minted join says no rate of its own yet, and then it is its first
    /// part's -- which for every join of one session's takes is that session's.
    #[test]
    fn a_minted_join_takes_the_rate_of_what_it_is_made_of() {
        let mut source = join(vec![part(1, Some((0, 100)))], Some(1));
        source.sample_rate = None;
        let table = HashMap::from([(
            SourceId(1),
            Held {
                buffer: 7,
                channels: 1,
                frames: 200,
                rate: 44_100.0,
            },
        )]);
        let made = stitch(&source, &table).expect("its part is there");
        assert_eq!(made.rate, 44_100.0);
        assert_eq!(made.parts[0].frames, 100, "so nothing crosses anything");
    }

    /// The ordinary join: two spans of one take, back to back.
    #[test]
    fn a_join_is_its_parts_in_order_and_as_long_as_their_sum() {
        let source = join(
            vec![part(1, Some((0, 100))), part(1, Some((400, 500)))],
            None,
        );
        let made = stitch(&source, &held(&[(1, 7, 2, 96_000)])).expect("both parts are there");
        assert_eq!(made.channels, 2, "the take's own width");
        assert_eq!(made.frames, 200);
        assert_eq!(made.parts[0].start, 0);
        assert_eq!(made.parts[1].start, 400);
        assert!(made.parts.iter().all(|p| p.buffer == 7 && p.frames == 100));
        assert_eq!(made.parts[0].channels, [0, 1], "straight through");
    }

    /// **A part narrower than the join repeats.** A mono take in a stereo join
    /// is heard on both sides rather than on one, which is the half of this
    /// that had drifted between the two places it was written.
    #[test]
    fn a_narrow_part_fills_the_join_rather_than_half_of_it() {
        let source = join(vec![part(1, Some((0, 100))), part(2, Some((0, 50)))], None);
        let made = stitch(&source, &held(&[(1, 7, 2, 96_000), (2, 8, 1, 50)])).expect("resolved");
        assert_eq!(made.channels, 2, "the widest part decides");
        assert_eq!(made.parts[0].channels, [0, 1]);
        assert_eq!(
            made.parts[1].channels,
            [0, 0],
            "the mono take on both sides"
        );
    }

    /// A part with no range is the whole of its source, which only the caller
    /// knows the length of.
    #[test]
    fn a_part_with_no_range_is_the_whole_take() {
        let source = join(vec![part(1, None)], None);
        let made = stitch(&source, &held(&[(1, 7, 1, 4_800)])).expect("resolved");
        assert_eq!((made.parts[0].start, made.parts[0].frames), (0, 4_800));
        assert_eq!(made.frames, 4_800);
    }

    /// **Unmade rather than half made.** A part whose source nobody has loaded
    /// leaves the whole join unresolved: a box over it draws empty and plays
    /// nothing, which is what a source nobody answered for has always meant.
    #[test]
    fn a_part_nobody_resolved_leaves_the_join_unmade() {
        let source = join(vec![part(1, Some((0, 100))), part(9, Some((0, 100)))], None);
        assert_eq!(stitch(&source, &held(&[(1, 7, 2, 96_000)])), None);
        // ...and it is not a join at all when the source is a file.
        let file = Source::file("take.wav", Lifetime::Session);
        assert_eq!(stitch(&file, &held(&[(1, 7, 2, 96_000)])), None);
    }
}
