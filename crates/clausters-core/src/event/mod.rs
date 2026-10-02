//! What an event's keys mean: the pitch, level and length an `Event` sounds,
//! and how the keys of one family stay coherent when one of them is edited.
//!
//! An event is a map of keys in every client -- a `dict` in Python, an object in
//! TypeScript -- and in the document. What is shared is not the map but the
//! **rules over it**, so this module takes the values of one family at a time
//! and never holds an event: a pattern makes thousands of events a second, and
//! none of them should need an object on this side to answer what it sounds.
//!
//! The families:
//!
//! - **pitch**: `freq` (Hz), `midinote`, and `degree` + `alter` within
//!   `octave`, `root` and `scale`. An explicit `freq` wins, else `midinote`,
//!   else the degree -- SuperCollider's order.
//! - **level**: `amp` (linear), `velocity` (MIDI, 1..127) and `db`. An explicit
//!   `amp` wins, else `velocity`, else `db`.
//! - **length**: `delta` and `sustain`, each an explicit key or derived from
//!   `dur`, `legato` and `stretch`.
//!
//! **Coherence.** A family's keys are several spellings of one quantity, so an
//! edit to one of them rewrites the others the event already holds: moving a
//! note's `midinote` in a roll updates the `freq` it was written with, and the
//! `degree` and `alter` too when it was written by degree. A key the event does
//! not hold is not added -- it is derived when asked for.
//!
//! **A degree is altered by its own key**, `alter`, in semitones (real, so a
//! microtone is one too), as MusicXML names it. SuperCollider writes the
//! alteration into the degree's fraction (`1.1` is degree 1 sharp); that
//! spelling is accepted and split into the two keys ([`split_degree`]), so
//! the arithmetic a pattern does on degrees stays on integers.
//!
//! [`render`] turns an event's map into what its destination plays: a synth's
//! messages, a MIDI port's, or nothing for a rest.
//!
//! The conversions are the core's own, reused: the equal-temperament pair
//! [`crate::scale::midi_to_hz`] / [`crate::scale::hz_to_midi`] (exact inverses, which is what
//! coherence needs), and the `ampdb` / `dbamp` unary operators the server runs.

pub mod render;

use crate::builtins::{UnaryOp, apply_unary};
use crate::scale::{hz_to_midi, midi_to_hz};

/// The MIDI note an event with no pitch key sounds: middle C.
pub const DEFAULT_MIDINOTE: f64 = 60.0;
/// The linear amplitude an event with no level key sounds.
pub const DEFAULT_AMP: f64 = 0.1;
/// The octave a degree is read in when none is stated.
pub const DEFAULT_OCTAVE: f64 = 5.0;
/// The major scale, the one a degree indexes when none is stated.
pub const MAJOR: [f32; 7] = [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0];

/// Which accidental a note written by degree takes when it falls between two
/// degrees of its scale -- the `spelling` notation key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spelling {
    /// The degree below, raised. Also what an event that says nothing gets.
    #[default]
    Sharp,
    /// The degree above, lowered.
    Flat,
}

impl Spelling {
    /// The spelling a door's integer names: negative is flat, anything else
    /// sharp (`0` being "not stated").
    #[must_use]
    pub fn from_i32(value: i32) -> Self {
        if value < 0 { Self::Flat } else { Self::Sharp }
    }

    /// The spelling the `spelling` key's value names (`"sharp"` or `"flat"`);
    /// anything else, or nothing, is sharp.
    #[must_use]
    pub fn from_key(value: Option<&str>) -> Self {
        if value == Some("flat") {
            Self::Flat
        } else {
            Self::Sharp
        }
    }
}

/// A SuperCollider degree split into the two keys: the degree it rounds to,
/// and the alteration its fraction states in tenths of a semitone step -- `1.1`
/// is `(1, +1)`, `0.9` is `(1, -1)`, `2.0` is `(2, 0)`. SuperCollider's
/// `degreeToKey` reads it so (round, then the remainder times ten), which is
/// why the fraction spells at most four semitones either way.
#[must_use]
pub fn split_degree(degree: f64) -> (f64, f64) {
    let whole = (degree + 0.5).floor();
    // The remainder of a decimal fraction is not exact in binary (1.1 - 1 is
    // 0.10000000000000009): kept to a millionth of a semitone.
    let alter = ((degree - whole) * 10.0 * 1e6).round() / 1e6;
    (whole, alter)
}

/// Scale degree -> MIDI note number: `degree` indexes `scale` (semitone
/// offsets within one octave) in the pitch space `octave` / `root`, wrapping
/// with octave carry -- degree -1 on a seven-note scale is the seventh one
/// octave down (floored division, sclang semantics) -- and `alter` semitones
/// are added. A fractional `degree` is read as SuperCollider writes an
/// alteration ([`split_degree`]), on top of `alter`. An empty `scale` yields
/// middle C.
#[must_use]
pub fn degree_to_midinote(degree: f64, alter: f64, octave: f64, root: f64, scale: &[f32]) -> f64 {
    let n = scale.len() as i64;
    if n == 0 {
        return DEFAULT_MIDINOTE;
    }
    let (whole, written) = split_degree(degree);
    let d = whole as i64;
    let step = f64::from(scale[d.rem_euclid(n) as usize]);
    12.0 * octave + root + step + 12.0 * d.div_euclid(n) as f64 + alter + written
}

/// MIDI note number -> the degree and alteration that write it in the pitch
/// space `octave` / `root` / `scale`: the inverse of [`degree_to_midinote`],
/// what coherence uses when a note written by degree is moved. A note on the
/// scale is its degree with no alteration; one between two degrees is the one
/// below raised ([`Spelling::Sharp`]) or the one above lowered
/// ([`Spelling::Flat`]). An empty scale gives degree 0 altered by the distance
/// from middle C.
#[must_use]
pub fn midinote_to_degree(
    midinote: f64,
    octave: f64,
    root: f64,
    scale: &[f32],
    spelling: Spelling,
) -> (f64, f64) {
    let n = scale.len() as i64;
    if n == 0 {
        return (0.0, midinote - DEFAULT_MIDINOTE);
    }
    let at = |d: i64| degree_to_midinote(d as f64, 0.0, octave, root, scale);
    // A first guess an octave's worth of degrees wide, then walked to the two
    // degrees around the note -- a scale is ascending within its octave.
    let rel = midinote - 12.0 * octave - root;
    let mut d = (rel / 12.0).floor() as i64 * n;
    while at(d) > midinote {
        d -= 1;
    }
    while at(d + 1) <= midinote {
        d += 1;
    }
    let below = midinote - at(d);
    if below.abs() < 1e-9 {
        return (d as f64, 0.0);
    }
    match spelling {
        Spelling::Sharp => (d as f64, below),
        Spelling::Flat => ((d + 1) as f64, midinote - at(d + 1)),
    }
}

/// A key of the pitch family, as an edit names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitchKey {
    Freq,
    Midinote,
    Degree,
    Alter,
    Octave,
    Root,
    /// The scale itself: its value is the `scale` passed beside the edit.
    Scale,
}

impl PitchKey {
    /// The key a door's index names, in [`Pitch::to_array`]'s order with
    /// `scale` last.
    #[must_use]
    pub fn from_index(i: u32) -> Option<Self> {
        Some(match i {
            0 => Self::Freq,
            1 => Self::Midinote,
            2 => Self::Degree,
            3 => Self::Alter,
            4 => Self::Octave,
            5 => Self::Root,
            6 => Self::Scale,
            _ => return None,
        })
    }

    /// The key an event spells `name` with, if it is one of this family.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "freq" => Self::Freq,
            "midinote" => Self::Midinote,
            "degree" => Self::Degree,
            "alter" => Self::Alter,
            "octave" => Self::Octave,
            "root" => Self::Root,
            "scale" => Self::Scale,
            _ => return None,
        })
    }
}

/// The pitch keys an event holds, each `None` when it does not hold it. The
/// scale is passed beside it, since it is a list rather than a number.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pitch {
    pub freq: Option<f64>,
    pub midinote: Option<f64>,
    pub degree: Option<f64>,
    pub alter: Option<f64>,
    pub octave: Option<f64>,
    pub root: Option<f64>,
}

fn present(value: f64) -> Option<f64> {
    (!value.is_nan()).then_some(value)
}

fn absent(value: Option<f64>) -> f64 {
    value.unwrap_or(f64::NAN)
}

impl Pitch {
    /// The keys as a door carries them -- `[freq, midinote, degree, alter,
    /// octave, root]`, NaN for a key the event does not hold.
    #[must_use]
    pub fn from_array(keys: [f64; 6]) -> Self {
        Self {
            freq: present(keys[0]),
            midinote: present(keys[1]),
            degree: present(keys[2]),
            alter: present(keys[3]),
            octave: present(keys[4]),
            root: present(keys[5]),
        }
    }

    /// The inverse of [`Pitch::from_array`].
    #[must_use]
    pub fn to_array(self) -> [f64; 6] {
        [
            absent(self.freq),
            absent(self.midinote),
            absent(self.degree),
            absent(self.alter),
            absent(self.octave),
            absent(self.root),
        ]
    }

    fn octave(&self) -> f64 {
        self.octave.unwrap_or(DEFAULT_OCTAVE)
    }

    fn root(&self) -> f64 {
        self.root.unwrap_or(0.0)
    }

    /// The note the degree keys write, when the event is written by degree.
    fn degree_note(&self, scale: &[f32]) -> Option<f64> {
        let degree = self.degree?;
        Some(degree_to_midinote(
            degree,
            self.alter.unwrap_or(0.0),
            self.octave(),
            self.root(),
            scale,
        ))
    }

    /// The MIDI note this event sounds: an explicit `freq` inverted, else
    /// `midinote`, else the degree, else middle C.
    #[must_use]
    pub fn midinote(&self, scale: &[f32]) -> f64 {
        if let Some(freq) = self.freq {
            return hz_to_midi(freq);
        }
        if let Some(midinote) = self.midinote {
            return midinote;
        }
        self.degree_note(scale).unwrap_or(DEFAULT_MIDINOTE)
    }

    /// The frequency in Hz this event sounds: an explicit `freq`, else its
    /// [`Pitch::midinote`] in equal temperament.
    #[must_use]
    pub fn freq(&self, scale: &[f32]) -> f64 {
        self.freq
            .unwrap_or_else(|| midi_to_hz(self.midinote(scale)))
    }

    /// Writes `key` and rewrites the other pitch keys the event holds, so they
    /// all still say the same note. `value` is ignored for [`PitchKey::Scale`],
    /// whose new value is `scale`. A fractional degree is split into `degree`
    /// and `alter` ([`split_degree`]); `spelling` picks the accidental when a
    /// moved note lands between two degrees.
    pub fn set(&mut self, key: PitchKey, value: f64, scale: &[f32], spelling: Spelling) {
        match key {
            PitchKey::Freq => {
                self.freq = Some(value);
                self.follow(hz_to_midi(value), key, scale, spelling);
            }
            PitchKey::Midinote => {
                self.midinote = Some(value);
                self.follow(value, key, scale, spelling);
            }
            PitchKey::Degree => {
                let (whole, written) = split_degree(value);
                self.degree = Some(whole);
                if written != 0.0 {
                    self.alter = Some(written);
                }
                self.lead_from_degree(scale);
            }
            PitchKey::Alter => {
                self.alter = Some(value);
                self.lead_from_degree(scale);
            }
            PitchKey::Octave => {
                self.octave = Some(value);
                self.lead_from_degree(scale);
            }
            PitchKey::Root => {
                self.root = Some(value);
                self.lead_from_degree(scale);
            }
            PitchKey::Scale => self.lead_from_degree(scale),
        }
    }

    /// The degree keys changed: the note they write goes to `midinote` and
    /// `freq`, where the event holds them.
    fn lead_from_degree(&mut self, scale: &[f32]) {
        if let Some(note) = self.degree_note(scale) {
            if self.midinote.is_some() {
                self.midinote = Some(note);
            }
            if self.freq.is_some() {
                self.freq = Some(midi_to_hz(note));
            }
        }
    }

    /// `note` was written through `key`: every other spelling the event holds
    /// follows it.
    fn follow(&mut self, note: f64, key: PitchKey, scale: &[f32], spelling: Spelling) {
        if key != PitchKey::Midinote && self.midinote.is_some() {
            self.midinote = Some(note);
        }
        if key != PitchKey::Freq && self.freq.is_some() {
            self.freq = Some(midi_to_hz(note));
        }
        if self.degree.is_some() {
            let (degree, alter) =
                midinote_to_degree(note, self.octave(), self.root(), scale, spelling);
            self.degree = Some(degree);
            self.alter = (alter != 0.0 || self.alter.is_some()).then_some(alter);
        }
    }
}

/// A key of the level family, as an edit names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelKey {
    Amp,
    Velocity,
    Db,
}

impl LevelKey {
    /// The key a door's index names, in [`Level::to_array`]'s order.
    #[must_use]
    pub fn from_index(i: u32) -> Option<Self> {
        Some(match i {
            0 => Self::Amp,
            1 => Self::Velocity,
            2 => Self::Db,
            _ => return None,
        })
    }

    /// The key an event spells `name` with, if it is one of this family.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "amp" => Self::Amp,
            "velocity" => Self::Velocity,
            "db" => Self::Db,
            _ => return None,
        })
    }
}

/// A MIDI velocity as the linear amplitude that goes with it (`v / 127`).
#[must_use]
pub fn amp_of_velocity(velocity: f64) -> f64 {
    (velocity / 127.0).clamp(0.0, 1.0)
}

/// A linear amplitude as the velocity a note-on carries: `amp * 127`, rounded,
/// and never below 1 -- a note-on at velocity 0 is a note-off.
#[must_use]
pub fn velocity_of_amp(amp: f64) -> f64 {
    (amp * 127.0).round().clamp(1.0, 127.0)
}

fn dbamp(db: f64) -> f64 {
    f64::from(apply_unary(UnaryOp::Dbamp, db as f32))
}

fn ampdb(amp: f64) -> f64 {
    f64::from(apply_unary(UnaryOp::Ampdb, amp as f32))
}

/// The level keys an event holds, each `None` when it does not hold it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Level {
    pub amp: Option<f64>,
    pub velocity: Option<f64>,
    pub db: Option<f64>,
}

impl Level {
    /// The keys as a door carries them -- `[amp, velocity, db]`, NaN for a key
    /// the event does not hold.
    #[must_use]
    pub fn from_array(keys: [f64; 3]) -> Self {
        Self {
            amp: present(keys[0]),
            velocity: present(keys[1]),
            db: present(keys[2]),
        }
    }

    /// The inverse of [`Level::from_array`].
    #[must_use]
    pub fn to_array(self) -> [f64; 3] {
        [absent(self.amp), absent(self.velocity), absent(self.db)]
    }

    /// The linear amplitude this event sounds at: an explicit `amp`, else its
    /// `velocity`, else its `db`, else [`DEFAULT_AMP`].
    #[must_use]
    pub fn amp(&self) -> f64 {
        if let Some(amp) = self.amp {
            return amp;
        }
        if let Some(velocity) = self.velocity {
            return amp_of_velocity(velocity);
        }
        self.db.map_or(DEFAULT_AMP, dbamp)
    }

    /// The velocity a note-on of this event carries: an explicit `velocity`
    /// (rounded into 1..127), else its amplitude's.
    #[must_use]
    pub fn velocity(&self) -> f64 {
        match self.velocity {
            Some(velocity) => velocity.round().clamp(1.0, 127.0),
            None => velocity_of_amp(self.amp()),
        }
    }

    /// Writes `key` and rewrites the other level keys the event holds.
    pub fn set(&mut self, key: LevelKey, value: f64) {
        let amp = match key {
            LevelKey::Amp => {
                self.amp = Some(value);
                value
            }
            LevelKey::Velocity => {
                self.velocity = Some(value);
                amp_of_velocity(value)
            }
            LevelKey::Db => {
                self.db = Some(value);
                dbamp(value)
            }
        };
        if key != LevelKey::Amp && self.amp.is_some() {
            self.amp = Some(amp);
        }
        if key != LevelKey::Velocity && self.velocity.is_some() {
            self.velocity = Some(velocity_of_amp(amp));
        }
        if key != LevelKey::Db && self.db.is_some() {
            self.db = Some(ampdb(amp));
        }
    }
}

/// Beats until the next event: an explicit `delta`, else `dur * stretch`.
#[must_use]
pub fn delta(dur: f64, stretch: f64, delta: Option<f64>) -> f64 {
    delta.unwrap_or(dur * stretch)
}

/// Beats the event sounds: an explicit `sustain`, else
/// `dur * legato * stretch`.
#[must_use]
pub fn sustain(dur: f64, legato: f64, stretch: f64, sustain: Option<f64>) -> f64 {
    sustain.unwrap_or(dur * legato * stretch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn degree_wraps_with_octave_carry() {
        assert_eq!(degree_to_midinote(0.0, 0.0, 5.0, 0.0, &MAJOR), 60.0);
        assert_eq!(degree_to_midinote(7.0, 0.0, 5.0, 0.0, &MAJOR), 72.0);
        assert_eq!(degree_to_midinote(-1.0, 0.0, 5.0, 0.0, &MAJOR), 59.0);
        assert_eq!(degree_to_midinote(1.0, 0.0, 5.0, 2.0, &MAJOR), 64.0);
        assert_eq!(degree_to_midinote(3.0, 0.0, 5.0, 0.0, &[]), 60.0);
    }

    /// The fraction was dropped, and a negative one truncated toward zero:
    /// `-0.9` read as degree 0 where SuperCollider reads degree -1 sharp.
    #[test]
    fn a_fractional_degree_is_an_alteration_not_a_truncation() {
        assert_eq!(degree_to_midinote(1.1, 0.0, 5.0, 0.0, &MAJOR), 63.0); // D#
        assert_eq!(degree_to_midinote(0.9, 0.0, 5.0, 0.0, &MAJOR), 61.0); // Db
        assert_eq!(degree_to_midinote(-0.9, 0.0, 5.0, 0.0, &MAJOR), 60.0); // B#, down
        assert_eq!(degree_to_midinote(-1.1, 0.0, 5.0, 0.0, &MAJOR), 58.0); // Bb, down
        assert_eq!(degree_to_midinote(1.0, 1.0, 5.0, 0.0, &MAJOR), 63.0);
        assert_eq!(degree_to_midinote(1.0, 0.5, 5.0, 0.0, &MAJOR), 62.5);
    }

    #[test]
    fn split_degree_is_supercolliders_reading() {
        assert_eq!(split_degree(1.1), (1.0, 1.0));
        assert_eq!(split_degree(0.9), (1.0, -1.0));
        assert_eq!(split_degree(2.0), (2.0, 0.0));
        assert_eq!(split_degree(-0.9), (-1.0, 1.0));
        assert_eq!(split_degree(1.4), (1.0, 4.0));
    }

    #[test]
    fn midinote_to_degree_inverts_and_spells() {
        let at = |m, s| midinote_to_degree(m, 5.0, 0.0, &MAJOR, s);
        assert_eq!(at(64.0, Spelling::Sharp), (2.0, 0.0));
        assert_eq!(at(61.0, Spelling::Sharp), (0.0, 1.0));
        assert_eq!(at(61.0, Spelling::Flat), (1.0, -1.0));
        assert_eq!(at(59.0, Spelling::Sharp), (-1.0, 0.0));
        assert_eq!(at(73.0, Spelling::Sharp), (7.0, 1.0));
        let (d, a) = at(60.5, Spelling::Sharp);
        assert_eq!(d, 0.0);
        assert!(close(a, 0.5));
        for m in 40..90 {
            for s in [Spelling::Sharp, Spelling::Flat] {
                let (d, a) = at(f64::from(m), s);
                assert!(close(
                    degree_to_midinote(d, a, 5.0, 0.0, &MAJOR),
                    f64::from(m)
                ));
            }
        }
    }

    #[test]
    fn resolution_order_is_freq_midinote_degree() {
        let pitch = Pitch {
            freq: Some(440.0),
            midinote: Some(60.0),
            ..Pitch::default()
        };
        assert!(close(pitch.midinote(&MAJOR), 69.0));
        let pitch = Pitch {
            midinote: Some(62.0),
            degree: Some(4.0),
            ..Pitch::default()
        };
        assert_eq!(pitch.midinote(&MAJOR), 62.0);
        let pitch = Pitch {
            degree: Some(4.0),
            ..Pitch::default()
        };
        assert_eq!(pitch.midinote(&MAJOR), 67.0);
        assert_eq!(Pitch::default().midinote(&MAJOR), 60.0);
        assert!(close(Pitch::default().freq(&MAJOR), midi_to_hz(60.0)));
    }

    /// The defect this closes: a moved `midinote` under an explicit `freq`
    /// sounded the old note, since `freq` wins.
    #[test]
    fn editing_one_pitch_key_rewrites_the_ones_held() {
        let mut pitch = Pitch {
            freq: Some(440.0),
            midinote: Some(69.0),
            ..Pitch::default()
        };
        pitch.set(PitchKey::Midinote, 72.0, &MAJOR, Spelling::Sharp);
        assert!(close(pitch.freq.unwrap(), midi_to_hz(72.0)));
        assert!(close(pitch.midinote(&MAJOR), 72.0));

        pitch.set(PitchKey::Freq, 440.0, &MAJOR, Spelling::Sharp);
        assert!(close(pitch.midinote.unwrap(), 69.0));
        assert_eq!(
            pitch.degree, None,
            "a key the event does not hold is not added"
        );

        let mut by_degree = Pitch {
            degree: Some(2.0),
            ..Pitch::default()
        };
        by_degree.set(PitchKey::Midinote, 66.0, &MAJOR, Spelling::Sharp);
        assert_eq!((by_degree.degree, by_degree.alter), (Some(3.0), Some(1.0)));
        by_degree.set(PitchKey::Midinote, 65.0, &MAJOR, Spelling::Sharp);
        assert_eq!(
            (by_degree.degree, by_degree.alter),
            (Some(3.0), Some(0.0)),
            "an alteration the event held stays a key, at zero"
        );

        let mut held = Pitch {
            degree: Some(0.0),
            midinote: Some(60.0),
            ..Pitch::default()
        };
        held.set(PitchKey::Octave, 4.0, &MAJOR, Spelling::Sharp);
        assert_eq!(held.midinote, Some(48.0));
        held.set(PitchKey::Degree, 1.1, &MAJOR, Spelling::Sharp);
        assert_eq!(
            (held.degree, held.alter, held.midinote),
            (Some(1.0), Some(1.0), Some(51.0))
        );
    }

    #[test]
    fn level_resolves_and_stays_coherent() {
        assert_eq!(Level::default().amp(), DEFAULT_AMP);
        assert_eq!(Level::default().velocity(), 13.0);
        let level = Level {
            velocity: Some(100.0),
            ..Level::default()
        };
        assert!(close(level.amp(), 100.0 / 127.0));

        let mut level = Level {
            amp: Some(0.1),
            ..Level::default()
        };
        level.set(LevelKey::Velocity, 100.0);
        assert!(close(level.amp.unwrap(), 100.0 / 127.0));
        level.set(LevelKey::Amp, 0.5);
        assert_eq!(level.velocity, Some(64.0));
        assert_eq!(level.db, None);
        assert_eq!(velocity_of_amp(0.0), 1.0, "a note-on is never a note-off");
        assert_eq!(velocity_of_amp(2.0), 127.0);
    }

    #[test]
    fn length_keys_override_the_calculation() {
        assert_eq!(delta(1.0, 2.0, None), 2.0);
        assert_eq!(delta(1.0, 2.0, Some(0.5)), 0.5);
        assert_eq!(sustain(1.0, 0.8, 2.0, None), 1.6);
        assert_eq!(sustain(1.0, 0.8, 2.0, Some(3.0)), 3.0);
    }

    #[test]
    fn doors_carry_nan_for_an_absent_key() {
        let pitch = Pitch::from_array([f64::NAN, 64.0, f64::NAN, f64::NAN, 5.0, f64::NAN]);
        assert_eq!(pitch.midinote, Some(64.0));
        assert_eq!(pitch.freq, None);
        let back = pitch.to_array();
        assert!(back[0].is_nan() && back[1] == 64.0);
    }
}
