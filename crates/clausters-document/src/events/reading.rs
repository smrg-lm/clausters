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

/// **The written value of `beats`**, in whole notes, as `[n, d]`, counted in
/// `beat_unit` beats a whole note -- `None` where no written value is that
/// long (a plain or dotted value, or a tuplet's of 3, 5, 6 or 7).
#[must_use]
pub fn written_value(beats: f64, beat_unit: i64) -> Option<Value> {
    let whole = beats / beat_unit.max(1) as f64;
    if whole <= 0.0 {
        return None;
    }
    for base in [1_i64, 3, 5, 7] {
        let mut d = base;
        while d <= 1024 * base {
            let n = (whole * d as f64).round();
            if n >= 1.0 && (n / d as f64 - whole).abs() < 1e-9 {
                let n = n as i64;
                let g = gcd(n, d);
                return Some(serde_json::json!([n / g, d / g]));
            }
            d *= 2;
        }
    }
    None
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs().max(1)
    } else {
        gcd(b, a % b)
    }
}
