//! **How a sequence is read into a page**: the options a reading decides by,
//! and the ones a sequence stores so that every view that opens it as a page
//! reads it alike (`notation.reading`).
//!
//! The options are plain data, and are not behind the `notation` feature: a
//! sequence checks the reading it is given whatever is compiled in, and the
//! reading itself is `transcription::read`'s.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::EventSequence;

/// **How a sequence is read into a score**: what the events do not say and a
/// reader has to. Every field has a default, so `{}` is a reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transcription {
    /// The meter, as `"3/4"`. Left out, the sequence's own where it was
    /// rendered from a score, else `4/4`.
    pub meter: Option<String>,
    /// The key, as a tonic name (`"F"`, `"Bb"`). Left out, the sequence's own,
    /// else the signature most of its notes are in.
    pub key: Option<String>,
    /// The clef of every staff the sequence does not say one for. Left out,
    /// each staff's is chosen by its register.
    pub clef: Option<String>,
    /// Which written value is one beat, as its denominator: `4` for a quarter.
    pub beat_unit: i64,
    /// The smallest written value an onset is snapped to, as its denominator:
    /// `16` for a sixteenth.
    pub division: i64,
    /// The tuplets a beat may be read as, each the parts it is divided in:
    /// `[3]` admits triplets. Empty, a beat is read in `division` alone --
    /// but for the tuplets the events themselves state in their `value`.
    pub tuplets: Vec<i64>,
    /// The most voices found on one staff.
    pub voices: usize,
    /// Whether levels are read back as dynamics and hairpins.
    pub dynamics: bool,
}

impl Default for Transcription {
    fn default() -> Self {
        Self {
            meter: None,
            key: None,
            clef: None,
            beat_unit: 4,
            division: 16,
            tuplets: Vec::new(),
            voices: 2,
            dynamics: true,
        }
    }
}

impl Transcription {
    /// **The reading a sequence states** (`notation.reading`), any option it
    /// leaves out at its default -- and the default reading where it states
    /// none, or one that does not read.
    #[must_use]
    pub fn of(sequence: &EventSequence) -> Self {
        sequence
            .reading()
            .and_then(|reading| serde_json::from_value(reading.clone()).ok())
            .unwrap_or_default()
    }

    /// **The sequence's reading with `given` over it**: each option `given`
    /// names in place of the sequence's, and every other one the sequence's,
    /// else its default -- so `{}` is the sequence's own reading, and an
    /// opener that asks for another meter keeps the rest of it.
    ///
    /// # Errors
    /// When `given` is not an object, or an option in it does not read.
    pub fn over(sequence: &EventSequence, given: &Value) -> Result<Self, String> {
        let Some(given) = given.as_object() else {
            return match given {
                Value::Null => Ok(Self::of(sequence)),
                _ => Err("a reading is written as an object of its options".into()),
            };
        };
        let mut options: Map<String, Value> = sequence
            .reading()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        options.extend(given.iter().map(|(k, v)| (k.clone(), v.clone())));
        serde_json::from_value(Value::Object(options))
            .map_err(|e| format!("the transcription could not be read: {e}"))
    }
}
