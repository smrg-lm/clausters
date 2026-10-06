//! **A played event's curves, as the values a client sends.**
//!
//! A sequence's curves reach its notes on the server: the lane samples each
//! into a table and a reader beside the voice follows it ([`crate::note_curves`]).
//! An event a client plays -- from a pattern, a routine, a timeline -- has no
//! lane, and it knows which of its controls have a curve only when it is
//! played. So nothing is built for it: the note is the plain synth it always
//! was, and the client sets the controls its curves drive, one value every
//! block, in timed bundles.
//!
//! [`window`] is that rule, once, for every client: given what sounds and a
//! stretch of time, the `/node_set`s of each instant in it. It reads a curve
//! through the function the lane's tables are sampled with
//! (`clausters_document::multitrack::nodes::value_at`), one value every
//! `step`, and it combines the two scopes as the lane's graphs do
//! (`clausters_core::event_graph`):
//!
//! - **a note's own curve** runs in the note's own time and wins over the
//!   channel's on the same control;
//! - **a channel's curve** is glided as the lane's reader glides it, by the
//!   same one-pole recurrence over [`CURVE_LAG`], once a step;
//! - **a bend** is the channel's plus the note's, in semitones, onto the
//!   frequency the note was started with.
//!
//! **A curve reaches a note from its start to its off**, and the note keeps
//! the last value through its release. The lane's readers follow a note's own
//! curve to the end of its release, because they are freed with the voice; a
//! client is not told when a voice ends, and a `/node_set` to a node that is
//! gone is a `/fail` -- and, offline, the end of the render. The off is the
//! last instant a client knows the node is there.
//!
//! It keeps no state. What has to outlive a window -- a channel curve's glide
//! -- goes out in the answer and comes back in the next request, so a client
//! holds plain data and a list.

use serde::Deserialize;
use serde_json::{Value, json};

use clausters_core::event_graph::{BEND, CURVE_LAG, FREQ};
use clausters_core::mixer::CURVE_STEP;
use clausters_document::Point;
use clausters_document::multitrack::nodes::value_at;

use crate::note_curves::{curve_channel, curve_control};

/// `ln(0.001)`: the glide converges to within -60 dB over the lag, as the
/// server's `Lag` does.
const LOG001: f64 = -6.907_755_278_982_137;

/// One curve: what it drives and its break points, on its own axis.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Curve {
    pub target: Value,
    pub points: Vec<Point>,
}

/// **A sounding note.** `start` and `off` are on the window's axis, in
/// seconds; `at` is where its own curves are at the window's `from` and
/// `rate` how fast they advance, in the curves' units a second -- a tempo,
/// read off the clock for this window.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Note {
    pub node: i32,
    pub start: f64,
    /// When it is released, and the last of its curves; `None` while that is
    /// not known.
    pub off: Option<f64>,
    pub at: f64,
    pub rate: f64,
    pub channel: i64,
    /// The frequency it was started with, which a bend multiplies.
    pub freq: Option<f64>,
    pub curves: Vec<Curve>,
}

/// **A channel's curve, playing.** `start` is on the window's axis; `at` and
/// `rate` place it at the window's `from`, as a note's are. `state` is the
/// glide where the last window left it, `None` to begin from the curve's own
/// value.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: i64,
    pub start: f64,
    pub at: f64,
    pub rate: f64,
    pub target: Value,
    pub points: Vec<Point>,
    pub state: Option<f64>,
}

/// What [`window`] is asked: the instants `k * step` with `from < t <= to`,
/// or -- with `start` -- each note at the instant it starts, which is what
/// its controls are set to in the bundle that makes it.
///
/// **The step is not the caller's**: it is the lane's
/// (`clausters_core::mixer::CURVE_STEP` frames, one block) at the
/// `sample_rate` the server runs at, so both paths sample a curve as finely.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    pub sample_rate: f64,
    pub from: f64,
    pub to: f64,
    pub start: bool,
    pub notes: Vec<Note>,
    pub channels: Vec<Channel>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            from: 0.0,
            to: 0.0,
            start: false,
            notes: Vec::new(),
            channels: Vec::new(),
        }
    }
}

impl Request {
    /// Seconds between two values of a curve.
    pub fn step(&self) -> f64 {
        if self.sample_rate > 0.0 {
            CURVE_STEP / self.sample_rate
        } else {
            0.0
        }
    }
}

/// A channel curve readied for a window: the control it drives and the channel
/// it acts on (`None` for every one).
struct Reader<'a> {
    spec: &'a Channel,
    control: String,
    channel: Option<i64>,
    state: Option<f64>,
    /// Its value at the instant being computed, once it has started.
    now: Option<f64>,
    /// Whether it still moves there: before its last point, or gliding onto it.
    moving: bool,
}

impl Reader<'_> {
    fn reaches(&self, note: &Note) -> bool {
        self.channel.is_none_or(|c| c == note.channel)
    }

    fn at(&self, t: f64, from: f64) -> f64 {
        self.spec.at + (t - from) * self.spec.rate
    }

    /// Whether it has anything left to say from `t` on: it has not started,
    /// it is before its last point, or its glide has not landed.
    fn pending(&self, t: f64, from: f64) -> bool {
        if t < self.spec.start {
            return true;
        }
        let at = self.at(t, from);
        at <= last_at(&self.spec.points)
            || self
                .state
                .is_some_and(|y| !close(y, value_at(&self.spec.points, at)))
    }

    /// Advance to `t`, a step after the last: the curve's value there, glided.
    fn advance(&mut self, t: f64, from: f64, glide: f64) {
        self.now = None;
        self.moving = false;
        if t < self.spec.start {
            return;
        }
        let raw = value_at(&self.spec.points, self.at(t, from));
        let y = self.state.map_or(raw, |y| raw + glide * (y - raw));
        self.moving = self.pending(t, from) || !close(y, raw);
        self.state = Some(y);
        self.now = Some(y);
    }

    /// Its value at `t` with nothing advanced: where the glide stands, or the
    /// curve's own value before any step.
    fn peek(&mut self, t: f64, from: f64) {
        self.now = (t >= self.spec.start).then(|| {
            self.state
                .unwrap_or_else(|| value_at(&self.spec.points, self.at(t, from)))
        });
        self.moving = true;
    }
}

fn last_at(points: &[Point]) -> f64 {
    points.last().map_or(0.0, |p| p.at)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// A note's own curves, one per control: the first that names it, with points.
fn own(note: &Note) -> Vec<(String, &Curve)> {
    let mut out: Vec<(String, &Curve)> = Vec::new();
    for curve in note.curves.iter().filter(|c| !c.points.is_empty()) {
        if let Some(control) = curve_control(&curve.target)
            && !out.iter().any(|(c, _)| *c == control)
        {
            out.push((control, curve));
        }
    }
    out
}

/// The channel's curves that reach `note`, each control on the first reader
/// that names it.
fn reaching<'a, 'b>(note: &Note, readers: &'a [Reader<'b>]) -> Vec<&'a Reader<'b>> {
    let mut out: Vec<&Reader> = Vec::new();
    for reader in readers.iter().filter(|r| r.reaches(note)) {
        if !out.iter().any(|r| r.control == reader.control) {
            out.push(reader);
        }
    }
    out
}

/// The `/node_set`s of `note` with its own curves standing at `at` (`slack`
/// past a curve's end is still sent, so its last value lands).
fn sets(note: &Note, at: f64, slack: f64, readers: &[Reader], out: &mut Vec<Value>) {
    let own = own(note);
    let running = |curve: &Curve| at <= last_at(&curve.points) + slack;
    let mut bend = None;
    let mut bend_moves = false;
    for (control, curve) in &own {
        if control == BEND {
            bend = Some(value_at(&curve.points, at));
            bend_moves |= running(curve);
        } else if running(curve) {
            out.push(json!([note.node, control, value_at(&curve.points, at)]));
        }
    }
    for reader in reaching(note, readers) {
        let Some(value) = reader.now else {
            continue;
        };
        if reader.control == BEND {
            bend = Some(bend.unwrap_or(0.0) + value);
            bend_moves |= reader.moving;
        } else if reader.moving && !own.iter().any(|(c, _)| *c == reader.control) {
            out.push(json!([note.node, reader.control, value]));
        }
    }
    if let (Some(base), Some(semitones)) = (note.freq, bend)
        && bend_moves
    {
        out.push(json!([note.node, FREQ, base * (semitones / 12.0).exp2()]));
    }
}

/// **The values of one window**: `{"bundles": [[t, [[node, control, value],
/// ...]], ...], "states": [[channel id, glide], ...], "over": [node, ...],
/// "idle": bool}`.
///
/// `bundles` holds an entry per instant that sets anything, in order;
/// `states` is what the next window is handed back; `over` names the notes
/// whose off the window reached, to which nothing more is sent; and `idle`
/// says that nothing is left to send to the rest either -- every curve of
/// theirs is past its end -- until a note or a curve is added.
pub fn window(request: &Request) -> Value {
    let step = request.step();
    let glide = if step > 0.0 {
        (LOG001 * step / f64::from(CURVE_LAG)).exp()
    } else {
        0.0
    };
    let mut readers: Vec<Reader> = request
        .channels
        .iter()
        .filter(|c| !c.points.is_empty())
        .filter_map(|spec| {
            Some(Reader {
                spec,
                control: curve_control(&spec.target)?,
                channel: curve_channel(&spec.target),
                state: spec.state,
                now: None,
                moving: false,
            })
        })
        .collect();
    let open = |note: &Note, t: f64| note.off.is_none_or(|off| t < off);
    let mut bundles = Vec::new();
    if request.start {
        // Each note at its own start: what its controls are set to as it is
        // made. Nothing advances -- the glide is where the last window left it.
        for note in &request.notes {
            for reader in &mut readers {
                reader.peek(note.start, request.from);
            }
            let mut out = Vec::new();
            sets(note, 0.0, f64::INFINITY, &readers, &mut out);
            if !out.is_empty() {
                bundles.push(json!([note.start, out]));
            }
        }
    } else if step > 0.0 {
        let first = (request.from / step).floor() as i64 + 1;
        let last = (request.to / step).floor() as i64;
        for k in first..=last {
            let t = k as f64 * step;
            for reader in &mut readers {
                reader.advance(t, request.from, glide);
            }
            let mut out = Vec::new();
            for note in &request.notes {
                if t <= note.start || !open(note, t) {
                    continue;
                }
                let at = note.at + (t - request.from) * note.rate;
                sets(note, at, step * note.rate, &readers, &mut out);
            }
            if !out.is_empty() {
                bundles.push(json!([t, out]));
            }
        }
    }
    let (to, from) = (request.to, request.from);
    let over: Vec<i32> = request
        .notes
        .iter()
        .filter(|note| !open(note, to))
        .map(|note| note.node)
        .collect();
    let idle = request.notes.iter().filter(|n| open(n, to)).all(|note| {
        let at = note.at + (to - from) * note.rate;
        own(note)
            .iter()
            .all(|(_, curve)| at > last_at(&curve.points) + step * note.rate)
            && reaching(note, &readers)
                .iter()
                .all(|reader| !reader.pending(to, from))
    });
    json!({
        "bundles": bundles,
        "states": readers
            .iter()
            .filter_map(|r| r.state.map(|y| json!([r.spec.id, y])))
            .collect::<Vec<_>>(),
        "over": over,
        "idle": idle,
    })
}

/// [`window`] over JSON: the request as text in, the answer as text out, and
/// `{"error": ...}` for a request that does not read.
pub fn window_json(request: &str) -> String {
    match serde_json::from_str::<Request>(request) {
        Ok(request) => window(&request).to_string(),
        Err(error) => json!({ "error": error.to_string() }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rate whose step is a round millisecond.
    const RATE: f64 = CURVE_STEP * 1000.0;
    const STEP: f64 = 0.001;

    fn curve(target: Value, points: &[(f64, f64)]) -> Curve {
        Curve {
            target,
            points: points
                .iter()
                .map(|&(at, value)| Point {
                    at,
                    value,
                    data: Default::default(),
                })
                .collect(),
        }
    }

    fn note(curves: Vec<Curve>) -> Note {
        Note {
            node: 1000,
            start: 0.0,
            off: Some(1.0),
            at: 0.0,
            rate: 1.0,
            freq: Some(440.0),
            curves,
            ..Note::default()
        }
    }

    fn channel(target: Value, points: &[(f64, f64)]) -> Channel {
        let curve = curve(target, points);
        Channel {
            id: 7,
            rate: 1.0,
            target: curve.target,
            points: curve.points,
            ..Channel::default()
        }
    }

    fn run(request: &Request) -> Value {
        window(request)
    }

    /// The value set on `control` in the bundle at index `i`.
    fn set(answer: &Value, i: usize, control: &str) -> Option<f64> {
        answer["bundles"][i][1]
            .as_array()?
            .iter()
            .find(|s| s[1] == control)
            .and_then(|s| s[2].as_f64())
    }

    /// A note's own curve is one value a step, on the grid, and only over the
    /// control it names: the others are left as the note was started.
    #[test]
    fn an_own_curve_sets_its_control_once_a_step() {
        let amp = curve(json!({"control": "amp"}), &[(0.0, 0.0), (0.01, 1.0)]);
        let answer = run(&Request {
            sample_rate: RATE,
            to: 0.005,
            notes: vec![note(vec![amp])],
            ..Request::default()
        });
        let bundles = answer["bundles"].as_array().unwrap();
        assert_eq!(bundles.len(), 5, "the instants 1..5 ms");
        assert!((bundles[0][0].as_f64().unwrap() - 0.001).abs() < 1e-12);
        assert!((set(&answer, 0, "amp").unwrap() - 0.1).abs() < 1e-5);
        assert!((set(&answer, 4, "amp").unwrap() - 0.5).abs() < 1e-5);
        assert_eq!(bundles[0][1].as_array().unwrap().len(), 1, "amp alone");
        assert_eq!(bundles[0][1][0][0], 1000);
    }

    /// Past its last point a curve says one value: it lands, and then nothing
    /// more is sent, so the window is idle. A note's off is the last of its
    /// curves, and from there it is over.
    #[test]
    fn a_curve_past_its_end_sends_nothing_and_a_released_note_is_over() {
        let amp = curve(json!({"control": "amp"}), &[(0.0, 0.0), (0.002, 1.0)]);
        let answer = run(&Request {
            sample_rate: RATE,
            to: 0.010,
            notes: vec![note(vec![amp.clone()])],
            ..Request::default()
        });
        let bundles = answer["bundles"].as_array().unwrap();
        assert_eq!(bundles.len(), 3, "1, 2 and the one that lands the end");
        assert_eq!(set(&answer, 2, "amp"), Some(1.0));
        assert_eq!(answer["over"], json!([]));
        assert_eq!(answer["idle"], true);

        let long = curve(json!({"control": "amp"}), &[(0.0, 0.0), (1.0, 1.0)]);
        let mut released = note(vec![long]);
        released.off = Some(0.0035);
        let answer = run(&Request {
            sample_rate: RATE,
            to: 0.010,
            notes: vec![released],
            ..Request::default()
        });
        assert_eq!(
            answer["bundles"].as_array().unwrap().len(),
            3,
            "nothing is sent from the off on"
        );
        assert_eq!(answer["over"], json!([1000]));
    }

    /// The bundle that makes a note carries its curves' first values, so the
    /// control never sounds the value the event was written with.
    #[test]
    fn a_note_starts_at_its_curves_first_values() {
        let amp = curve(json!({"control": "amp"}), &[(0.0, 0.25), (1.0, 1.0)]);
        let bend = curve(json!({"bend": true}), &[(0.0, 12.0), (1.0, 0.0)]);
        let mut starting = note(vec![amp, bend]);
        starting.start = 0.5;
        let answer = run(&Request {
            sample_rate: RATE,
            from: 0.5,
            to: 0.5,
            start: true,
            notes: vec![starting],
            ..Request::default()
        });
        assert_eq!(answer["bundles"][0][0], 0.5);
        assert_eq!(set(&answer, 0, "amp"), Some(0.25));
        assert!((set(&answer, 0, "freq").unwrap() - 880.0).abs() < 1e-5);
    }

    /// **A glissando moves through pitch, so through frequency by ratios.**
    /// A bend is in semitones and runs straight in time, and a semitone is a
    /// ratio: halfway up an octave's slide the note is a tritone up -- the
    /// geometric mean of the two frequencies, 622 Hz from 440 -- and never
    /// the arithmetic one, 660, which is what a slide drawn straight in
    /// hertz would play, too high all the way and heard as a sag at the end.
    #[test]
    fn a_glissando_is_straight_in_pitch_and_geometric_in_frequency() {
        let octave = curve(json!({"bend": true}), &[(0.0, 0.0), (1.0, 12.0)]);
        let mut sliding = note(vec![octave]);
        sliding.off = Some(2.0);
        let answer = run(&Request {
            sample_rate: RATE,
            to: 0.5,
            notes: vec![sliding],
            ..Request::default()
        });
        let bundles = answer["bundles"].as_array().unwrap();
        for (at, expect) in [(0.25, 440.0 * 2f64.powf(0.25)), (0.5, 440.0 * 2f64.sqrt())] {
            let i = bundles
                .iter()
                .position(|b| (b[0].as_f64().unwrap() - at).abs() < 1e-9)
                .expect("a bundle there");
            let freq = set(&answer, i, "freq").unwrap();
            assert!((freq - expect).abs() < 1e-3, "{at}: {freq} for {expect}");
        }
    }

    /// A channel's curve reaches the notes of its channel while they are
    /// open, a note's own curve over the same control wins, and a released
    /// note is left with what it had.
    #[test]
    fn a_channel_curve_reaches_its_open_notes() {
        let level = channel(
            json!({"control": "amp", "channel": 2}),
            &[(0.0, 0.5), (1.0, 0.5)],
        );
        let mut on_it = note(vec![]);
        on_it.channel = 2;
        let mut elsewhere = note(vec![]);
        (elsewhere.node, elsewhere.channel) = (1001, 3);
        let mut own_amp = note(vec![curve(
            json!({"control": "amp"}),
            &[(0.0, 0.75), (1.0, 0.75)],
        )]);
        (own_amp.node, own_amp.channel) = (1002, 2);
        let mut released = note(vec![]);
        (released.node, released.channel, released.off) = (1003, 2, Some(0.0005));
        let answer = run(&Request {
            sample_rate: RATE,
            to: 0.001,
            notes: vec![on_it, elsewhere, own_amp, released],
            channels: vec![level],
            ..Request::default()
        });
        let sets = answer["bundles"][0][1].as_array().unwrap();
        let of = |node: i64| -> Vec<f64> {
            sets.iter()
                .filter(|s| s[0] == node)
                .map(|s| s[2].as_f64().unwrap())
                .collect()
        };
        assert_eq!(of(1000), vec![0.5], "the channel's value");
        assert!(of(1001).is_empty(), "another channel");
        assert_eq!(of(1002), vec![0.75], "its own curve wins");
        assert!(of(1003).is_empty(), "released: it keeps what it had");
        assert_eq!(answer["states"], json!([[7, 0.5]]));
    }

    /// A channel's curve is glided as the lane's reader glides it: a jump is
    /// approached over the lag, and the glide is handed on to the next window.
    #[test]
    fn a_channel_curve_is_glided_across_windows() {
        let mut jump = channel(json!({"control": "amp"}), &[(0.0, 1.0)]);
        jump.state = Some(0.0);
        let mut request = Request {
            sample_rate: RATE,
            to: 0.002,
            notes: vec![note(vec![])],
            channels: vec![jump],
            ..Request::default()
        };
        let first = run(&request);
        let glide = (LOG001 * STEP / f64::from(CURVE_LAG)).exp();
        let one = 1.0 - glide;
        assert!((set(&first, 0, "amp").unwrap() - one).abs() < 1e-12);
        let two = 1.0 - glide * glide;
        assert!((first["states"][0][1].as_f64().unwrap() - two).abs() < 1e-12);
        // The next window goes on from there.
        request.channels[0].state = first["states"][0][1].as_f64();
        (request.from, request.to) = (0.002, 0.003);
        request.channels[0].at = 0.002;
        let second = run(&request);
        let three = 1.0 - glide * glide * glide;
        assert!((set(&second, 0, "amp").unwrap() - three).abs() < 1e-12);
    }

    /// A bend is the channel's plus the note's, onto the frequency the note
    /// was started with.
    #[test]
    fn a_bend_adds_the_two_scopes() {
        let whole = channel(json!({"bend": true}), &[(0.0, 2.0), (1.0, 2.0)]);
        let own_bend = curve(json!({"bend": true}), &[(0.0, 0.0), (0.004, 4.0)]);
        let answer = run(&Request {
            sample_rate: RATE,
            to: 0.002,
            notes: vec![note(vec![own_bend])],
            channels: vec![whole],
            ..Request::default()
        });
        let at = |semitones: f64| 440.0 * (semitones / 12.0_f64).exp2();
        assert!((set(&answer, 0, "freq").unwrap() - at(2.0 + 1.0)).abs() < 1e-3);
        assert!((set(&answer, 1, "freq").unwrap() - at(2.0 + 2.0)).abs() < 1e-3);
    }

    /// The position of a curve is the caller's: a tempo is a rate.
    #[test]
    fn a_curve_advances_at_the_rate_it_is_given() {
        let amp = curve(json!({"control": "amp"}), &[(0.0, 0.0), (1.0, 1.0)]);
        let mut fast = note(vec![amp]);
        (fast.at, fast.rate) = (0.5, 2.0);
        let answer = run(&Request {
            sample_rate: RATE,
            from: 0.25,
            to: 0.251,
            notes: vec![fast],
            ..Request::default()
        });
        let t = answer["bundles"][0][0].as_f64().unwrap();
        let expected = 0.5 + (t - 0.25) * 2.0;
        assert!((set(&answer, 0, "amp").unwrap() - expected).abs() < 1e-5);
    }

    #[test]
    fn a_request_that_does_not_read_says_so() {
        assert!(window_json("nope").contains("error"));
        assert_eq!(
            window_json("{}"),
            r#"{"bundles":[],"idle":true,"over":[],"states":[]}"#
        );
    }
}
