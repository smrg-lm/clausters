//! **The spectrogram's settings**: how the audio editor's spectrogram
//! analyzes the take and draws it, held by the editor for its window and
//! shown in a panel beside the take (`crate::panels`).
//!
//! The analysis is a window of a size and a shape every so many frames,
//! which the panel says as an overlap, as a phase vocoder's FFT does: `0` is
//! windows laid end to end, and each step after it halves the hop, to a
//! sixteenth of the window. The display is the frequency axis (a note axis at
//! a reference tuning among them), the colours and the levels they span (a
//! gain and a range in decibels), whether the picture is drawn smooth rather
//! than in blocks, and whether each band is drawn at its own scale. All of it
//! is a view state, outside the history, and it reaches the picture as the
//! spectrogram's own props.
//!
//! The panel's widgets are named here ([`names`]) and numbered by the caller
//! with the dialogs', so a window composed without them has no panel, and a
//! report from one is read here ([`Settings::read`]).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How wide the panel's column is.
pub const WIDTH: f64 = 220.0;

/// The prefix the panel's widgets are named under.
pub const PREFIX: &str = "settings";

/// The analysis windows' sizes, in frames.
pub const WINDOWS: &[usize] = &[256, 512, 1024, 2048, 4096];

/// The windows' shapes, by name and `wintype`, in the order offered.
pub const SHAPES: &[(&str, &str, i32)] = &[
    ("hann", "Hann", 0),
    ("hamming", "Hamming", 3),
    ("blackman", "Blackman", 4),
    ("sine", "Sine", 1),
    ("welch", "Welch", 2),
    ("rectangular", "Rectangular", -1),
];

/// **The overlaps offered**, as fractions of the window and as the panel
/// says them: each halves the hop of the one before. A hop that is the
/// window over a power of two lies on the grid of every coarser one and of
/// the power-of-two columns a view asks for, so the windows a zoom draws are
/// the ones already analyzed; a percent of a window lands on neither, and
/// past a sixteenth of the window a closer one shows nothing a pixel can.
pub const OVERLAPS: &[(f64, &str)] = &[
    (0.0, "0%"),
    (0.5, "50%"),
    (0.75, "75%"),
    (0.875, "87.5%"),
    (0.9375, "93.75%"),
];

/// The levels the colours span, in decibels, from the most contrast to the
/// least.
pub const RANGE_DB: (f64, f64) = (20.0, 120.0);

/// How far the top of the colours is lowered below full scale, in decibels.
pub const GAIN_DB: (f64, f64) = (0.0, 60.0);

/// The frequency axes, by name, in the order offered.
pub const SCALES: &[(&str, &str)] = &[
    ("linear", "Linear"),
    ("log", "Logarithmic"),
    ("mel", "Mel"),
    ("bark", "Bark"),
    ("note", "Notes"),
];

/// The colours, by name and index.
pub const COLORMAPS: &[(&str, &str)] = &[
    ("viridis", "Viridis"),
    ("magma", "Magma"),
    ("gray", "Grayscale"),
];

/// **How the spectrogram analyzes and draws the take.**
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The window, in frames.
    pub window: usize,
    /// The window's shape, by name ([`SHAPES`]).
    pub shape: String,
    /// How much a window overlaps the next, one of [`OVERLAPS`].
    pub overlap: f64,
    /// The frequency axis, by name ([`SCALES`]).
    pub scale: String,
    /// The reference A4 of a note axis, in hertz.
    pub tuning: f64,
    /// The colours, by name ([`COLORMAPS`]).
    pub colormap: String,
    /// The levels the colours span below their top, in decibels.
    pub range: f64,
    /// How far the top of the colours is below full scale, in decibels: a
    /// quiet take raised to the colours a loud one is drawn in.
    pub gain: f64,
    /// Drawn smooth between the bins and the columns, rather than in blocks.
    pub interpolate: bool,
    /// Each frequency band drawn at its own scale.
    pub normalize: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: 1024,
            shape: "hann".into(),
            overlap: 0.5,
            scale: "linear".into(),
            tuning: 440.0,
            colormap: "viridis".into(),
            range: 90.0,
            gain: 0.0,
            interpolate: false,
            normalize: false,
        }
    }
}

/// The widgets of the panel, by name.
pub fn names() -> Vec<String> {
    [
        "window",
        "shape",
        "overlap",
        "scale",
        "tuning",
        "colormap",
        "range",
        "gain",
        "interpolate",
        "normalize",
    ]
    .iter()
    .map(|n| format!("{PREFIX}:{n}"))
    .collect()
}

/// The caller's widget id for each widget of the panel it numbered.
pub type Ids = crate::forms::Ids;

impl Settings {
    /// **The hop between two windows**: the window less what overlaps, a
    /// frame at the least.
    pub fn hop(&self) -> usize {
        let hop = (self.window as f64 * (1.0 - self.overlap.clamp(0.0, 1.0))).round();
        (hop as usize).max(1)
    }

    fn wintype(&self) -> i32 {
        SHAPES
            .iter()
            .find(|(name, ..)| *name == self.shape)
            .map_or(0, |(.., w)| *w)
    }

    fn overlap_index(&self) -> usize {
        (0..OVERLAPS.len())
            .min_by(|&a, &b| {
                (OVERLAPS[a].0 - self.overlap)
                    .abs()
                    .total_cmp(&(OVERLAPS[b].0 - self.overlap).abs())
            })
            .unwrap_or(1)
    }

    fn colormap_index(&self) -> usize {
        COLORMAPS
            .iter()
            .position(|(name, _)| *name == self.colormap)
            .unwrap_or(0)
    }

    /// **The spectrogram's props** these settings come to.
    pub fn props(&self) -> Value {
        json!({
            "window_size": self.window,
            "hop": self.hop(),
            "wintype": self.wintype(),
            "freq_scale": self.scale,
            "tuning": self.tuning,
            "colormap": self.colormap_index(),
            "db_ceil": -self.gain,
            "db_floor": -self.gain - self.range,
            "interpolate": u8::from(self.interpolate),
            "normalize": u8::from(self.normalize),
        })
    }

    /// **Changes the settings a request names**, refusing a value none of
    /// them takes and changing nothing then.
    pub fn set(&mut self, request: &Value) -> Result<(), String> {
        let mut next = self.clone();
        if let Some(window) = request.get("window") {
            next.window = window
                .as_u64()
                .map(|w| w as usize)
                .filter(|w| WINDOWS.contains(w))
                .ok_or_else(|| format!("a window is one of {WINDOWS:?} frames"))?;
        }
        if let Some(shape) = request.get("shape") {
            next.shape = named(shape, SHAPES.iter().map(|(n, ..)| *n), "window shape")?;
        }
        if let Some(overlap) = request.get("overlap") {
            next.overlap = overlap
                .as_f64()
                .filter(|o| OVERLAPS.iter().any(|(f, _)| (f - o).abs() < 1e-9))
                .ok_or_else(|| {
                    let offered: Vec<f64> = OVERLAPS.iter().map(|(f, _)| *f).collect();
                    format!("an overlap is one of {offered:?}")
                })?;
        }
        if let Some(scale) = request.get("scale") {
            next.scale = named(scale, SCALES.iter().map(|(n, _)| *n), "frequency axis")?;
        }
        if let Some(tuning) = request.get("tuning") {
            next.tuning = tuning
                .as_f64()
                .filter(|t| (200.0..=1000.0).contains(t))
                .ok_or("a tuning is the A4 of 200 to 1000 Hz")?;
        }
        if let Some(colormap) = request.get("colormap") {
            next.colormap = named(colormap, COLORMAPS.iter().map(|(n, _)| *n), "colormap")?;
        }
        if let Some(range) = request.get("range") {
            next.range = range
                .as_f64()
                .filter(|r| (RANGE_DB.0..=RANGE_DB.1).contains(r))
                .ok_or_else(|| format!("a range is {RANGE_DB:?} dB"))?;
        }
        if let Some(gain) = request.get("gain") {
            next.gain = gain
                .as_f64()
                .filter(|g| (GAIN_DB.0..=GAIN_DB.1).contains(g))
                .ok_or_else(|| format!("a gain is {GAIN_DB:?} dB"))?;
        }
        if let Some(interpolate) = request.get("interpolate") {
            next.interpolate = interpolate
                .as_bool()
                .or_else(|| interpolate.as_f64().map(|n| n != 0.0))
                .ok_or("interpolate is on or off")?;
        }
        if let Some(normalize) = request.get("normalize") {
            next.normalize = normalize
                .as_bool()
                .or_else(|| normalize.as_f64().map(|n| n != 0.0))
                .ok_or("normalize is on or off")?;
        }
        *self = next;
        Ok(())
    }

    /// **What a report from the panel's widget `name` changes**, read as the
    /// request [`Settings::set`] takes: a choice reports the index of what was
    /// chosen, a field its figure and a switch its state. `None` for a report
    /// that is not a value.
    pub fn read(name: &str, value: &Value) -> Option<Value> {
        let part = name.strip_prefix(PREFIX)?.strip_prefix(':')?;
        let number = value
            .as_f64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
        let index = number.round().max(0.0) as usize;
        Some(match part {
            "window" => json!({"window": WINDOWS.get(index)?}),
            "shape" => json!({"shape": SHAPES.get(index)?.0}),
            "overlap" => json!({"overlap": OVERLAPS.get(index)?.0}),
            "scale" => json!({"scale": SCALES.get(index)?.0}),
            "tuning" => json!({"tuning": number}),
            "colormap" => json!({"colormap": COLORMAPS.get(index)?.0}),
            "range" => json!({"range": number.clamp(RANGE_DB.0, RANGE_DB.1)}),
            "gain" => json!({"gain": number.clamp(GAIN_DB.0, GAIN_DB.1)}),
            "interpolate" => json!({"interpolate": number != 0.0}),
            "normalize" => json!({"normalize": number != 0.0}),
            _ => return None,
        })
    }

    /// **The panel**, its widgets showing these settings, or `None` when the
    /// caller numbered none of them.
    pub fn panel(&self, ids: &Ids) -> Option<Value> {
        let id = |name: &str| ids.get(&format!("{PREFIX}:{name}")).copied();
        let choice = |name: &str, label: &str, options: Vec<&str>, index: usize| {
            id(name).map(|id| {
                json!({"type": "choice", "id": id, "label": label,
                    "options": options, "index": index})
            })
        };
        let window = WINDOWS.iter().position(|w| *w == self.window).unwrap_or(2);
        let shape = SHAPES
            .iter()
            .position(|(n, ..)| *n == self.shape)
            .unwrap_or(0);
        let scale = SCALES
            .iter()
            .position(|(n, _)| *n == self.scale)
            .unwrap_or(0);
        let field = |name: &str, label: &str, (min, max): (f64, f64), step: f64, value: f64| {
            id(name).map(|id| {
                json!({"type": "number", "id": id, "label": label, "stepper": true,
                    "min": min, "max": max, "step": step, "value": value})
            })
        };
        let toggle = |name: &str, label: &str, on: bool| {
            id(name).map(
                |id| json!({"type": "toggle", "id": id, "label": label, "value": u8::from(on)}),
            )
        };
        let analysis = vec![
            choice(
                "window",
                "Window size",
                vec!["256", "512", "1024", "2048", "4096"],
                window,
            ),
            choice(
                "shape",
                "Window type",
                SHAPES.iter().map(|(_, l, _)| *l).collect(),
                shape,
            ),
            choice(
                "overlap",
                "Overlap",
                OVERLAPS.iter().map(|(_, l)| *l).collect(),
                self.overlap_index(),
            ),
        ];
        let display = vec![
            choice(
                "scale",
                "Frequency scale",
                SCALES.iter().map(|(_, l)| *l).collect(),
                scale,
            ),
            field(
                "tuning",
                "Tuning (A\u{2084}, Hz)",
                (200.0, 1000.0),
                0.1,
                self.tuning,
            ),
            choice(
                "colormap",
                "Colors",
                COLORMAPS.iter().map(|(_, l)| *l).collect(),
                self.colormap_index(),
            ),
            field("range", "Range (dB)", RANGE_DB, 1.0, self.range),
            field("gain", "Gain (dB)", GAIN_DB, 1.0, self.gain),
            toggle("interpolate", "Interpolate", self.interpolate),
            toggle("normalize", "Normalize bands", self.normalize),
        ];
        // two sections, what is measured and how it is drawn, each folded
        // and unfolded whole
        let sections: Vec<Value> = [("Analysis", analysis), ("Display", display)]
            .into_iter()
            .filter_map(|(title, widgets)| {
                let widgets: Vec<Value> = widgets.into_iter().flatten().collect();
                (!widgets.is_empty()).then(|| {
                    let mut section = crate::panels::section(title, false, widgets);
                    section.insert("flow".into(), json!("col"));
                    Value::Object(section)
                })
            })
            .collect();
        crate::panels::column(sections, WIDTH)
    }
}

/// `value` as one of `names`, or why it is none of them.
fn named<'a>(
    value: &Value,
    mut names: impl Iterator<Item = &'a str>,
    what: &str,
) -> Result<String, String> {
    let word = value.as_str().ok_or_else(|| format!("a {what} is named"))?;
    names
        .any(|n| n == word)
        .then(|| word.to_string())
        .ok_or_else(|| format!("{word:?} is no {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, 500 + i as i32))
            .collect()
    }

    /// **The overlap is a phase vocoder's**: windows end to end at none, and
    /// each overlap offered halves the hop, to a sixteenth of the window. One
    /// of none of them is refused.
    #[test]
    fn an_overlap_is_the_window_less_its_hop() {
        let mut s = Settings::default();
        assert_eq!(s.hop(), 512);
        let hops: Vec<usize> = OVERLAPS
            .iter()
            .map(|(o, _)| {
                s.set(&json!({ "overlap": o })).unwrap();
                s.hop()
            })
            .collect();
        assert_eq!(hops, [1024, 512, 256, 128, 64]);
        assert!(s.set(&json!({"overlap": 0.6})).is_err());
        assert!(s.set(&json!({"overlap": 1.0})).is_err());
        assert_eq!(s.hop(), 64);
    }

    /// **The props are the spectrogram's own**, and a request is read whole or
    /// not at all.
    #[test]
    fn settings_come_to_the_spectrograms_props_and_a_bad_request_changes_nothing() {
        let mut s = Settings::default();
        s.set(
            &json!({"window": 2048, "shape": "blackman", "overlap": 0.75,
            "scale": "note", "tuning": 432.0, "colormap": "magma", "range": 60.0,
            "gain": 20.0, "interpolate": true, "normalize": true}),
        )
        .unwrap();
        assert_eq!(
            s.props(),
            json!({"window_size": 2048, "hop": 512, "wintype": 4, "freq_scale": "note",
                "tuning": 432.0, "colormap": 1, "db_ceil": -20.0, "db_floor": -80.0,
                "interpolate": 1, "normalize": 1})
        );
        let before = s.clone();
        assert!(
            s.set(&json!({"window": 2048, "shape": "triangle"}))
                .is_err()
        );
        assert!(s.set(&json!({"window": 3000})).is_err());
        assert!(s.set(&json!({"tuning": 50.0})).is_err());
        assert!(s.set(&json!({"range": 5.0})).is_err());
        assert!(s.set(&json!({"gain": -10.0})).is_err());
        assert_eq!(s, before);
    }

    /// **The panel shows the settings, and a widget's report is the request
    /// it comes to.**
    #[test]
    fn the_panel_shows_the_settings_and_a_report_is_read_as_a_request() {
        let mut s = Settings::default();
        let panel = s.panel(&ids()).expect("a panel");
        assert_eq!(panel["type"], "plane");
        let sections = panel["children"].as_array().unwrap();
        let titles: Vec<&Value> = sections.iter().map(|s| &s["title"]).collect();
        assert_eq!(titles, ["Analysis", "Display"]);
        let analysis = &sections[0]["children"];
        let display = &sections[1]["children"];
        assert_eq!(analysis.as_array().unwrap().len(), 3);
        assert_eq!(display.as_array().unwrap().len(), 7);
        assert_eq!(analysis[0]["index"], 2, "1024 frames");
        assert_eq!(analysis[2]["index"], 1, "half");
        assert_eq!(display[0]["index"], 0, "a linear scale");
        assert_eq!(display[3]["value"], 90.0, "the range");
        assert_eq!(
            Settings::read("settings:window", &json!(3)),
            Some(json!({"window": 2048}))
        );
        assert_eq!(
            Settings::read("settings:overlap", &json!(2)),
            Some(json!({"overlap": 0.75}))
        );
        assert_eq!(Settings::read("settings:overlap", &json!(5)), None);
        assert_eq!(
            Settings::read("settings:range", &json!(60.0)),
            Some(json!({"range": 60.0}))
        );
        assert_eq!(
            Settings::read("settings:interpolate", &json!(1)),
            Some(json!({"interpolate": true}))
        );
        assert_eq!(
            Settings::read("settings:scale", &json!("4")),
            Some(json!({"scale": "note"}))
        );
        assert_eq!(
            Settings::read("settings:normalize", &json!(1)),
            Some(json!({"normalize": true}))
        );
        assert_eq!(Settings::read("settings:window", &json!(9)), None);
        assert_eq!(Settings::read("dialog:new:ok", &json!(1)), None);
        s.set(&Settings::read("settings:shape", &json!(2)).unwrap())
            .unwrap();
        assert_eq!(s.shape, "blackman");
        // a window that numbered none has no panel
        assert!(s.panel(&Ids::new()).is_none());
    }
}
