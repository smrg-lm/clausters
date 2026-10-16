//! **A node's generic editor**: the GuiDef a host draws for any node from the
//! description of its def's controls, so a SynthDef, a FaustDef and a plugin
//! open the same kind of window without anybody writing one.
//!
//! The rule is here, once, and both clients call it (`clausters_core_node_editor`,
//! `node_editor` in the page): what a control becomes -- a slider, a slider with
//! detents, a toggle, a list -- what range it is drawn over when its def
//! declares none, how its value reads as text, and how a large def is grouped,
//! searched and paged. What it produces is an ordinary GuiDef: every control
//! widget carries an inline `bind` to `/node_set`, so the host writes the node
//! with no script in the loop, and every value's text is a label bound to
//! `/node_setText`; the host reads both back through the bindings' read forms
//! (`/synth_get`, `/synth_getText`).
//!
//! A control's description is what `/def_query` reports for it, the same
//! record in every family ([`Control`]). Its display metadata never limits the
//! control itself: a `/node_set` outside the range still sets it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::plugin_catalog::Parameter;

/// **How a control is drawn**, as a def declares it: optional, the range
/// whole or not at all, and checked when the def is built
/// ([`ControlDisplay::validate`]). A SynthDef control
/// spells it beside its default; a FaustDef's slider and a plugin's parameter
/// bring their own range.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlDisplay {
    /// The low end of the range it is drawn over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// The high end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// The grid a gesture lands on; zero or absent is continuous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// `"linear"` (the default) or `"exp"`, equal ratios for equal travel --
    /// what a frequency or a time wants. An exponential range is positive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warp: Option<String>,
    /// What the value is measured in (`"Hz"`, `"dB"`, `"s"`), written after
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl ControlDisplay {
    /// Refuses a description no view can draw: a bound that is not finite, a
    /// range that is empty or reversed, a negative step, a warp other than the
    /// two there are, or an exponential range that reaches zero.
    pub fn validate(&self) -> Result<(), String> {
        let finite = [self.min, self.max, self.step]
            .into_iter()
            .flatten()
            .all(f64::is_finite);
        if !finite {
            return Err("a control's min, max and step must be finite".into());
        }
        if self.min.is_some() != self.max.is_some() {
            return Err("a control's min and max are declared together".into());
        }
        if let (Some(lo), Some(hi)) = (self.min, self.max)
            && lo >= hi
        {
            return Err(format!(
                "a control's min ({lo}) must be below its max ({hi})"
            ));
        }
        if self.step.is_some_and(|s| s < 0.0) {
            return Err("a control's step must not be negative".into());
        }
        match self.warp.as_deref() {
            None | Some("linear") => {}
            Some("exp") if self.min.is_some_and(|lo| lo <= 0.0) => {
                return Err("an exponential control's range must be above zero".into());
            }
            Some("exp") => {}
            Some(other) => {
                return Err(format!(
                    "unknown warp {other:?}: a control is \"linear\" or \"exp\""
                ));
            }
        }
        Ok(())
    }
}

/// **One control of a def, described**: what `/def_query` reports for it, in
/// every family, and what a client hands the editor back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// The name `/node_set` takes.
    pub name: String,
    /// Its value when the node starts.
    pub default: f64,
    /// `"kr"`, `"tr"` (a trigger) or `"ir"` (fixed when the node starts).
    #[serde(default = "control_rate")]
    pub rate: String,
    /// How it is drawn.
    #[serde(flatten)]
    pub display: ControlDisplay,
    /// What a person reads for it, when that is not its name: a plugin
    /// parameter's own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The group it belongs to (a plugin's module); empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// The plugin flags ([`Parameter::flags`]); zero in the other families.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u32,
    /// What each step of a list is called, lowest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

fn control_rate() -> String {
    "kr".into()
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Control {
    /// A control with only a name and a default, as a SynthDef declares one.
    pub fn new(name: impl Into<String>, default: f64) -> Self {
        Self {
            name: name.into(),
            default,
            rate: control_rate(),
            display: ControlDisplay::default(),
            label: None,
            group: String::new(),
            flags: 0,
            labels: Vec::new(),
        }
    }

    /// What a person reads for it.
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// Whether a generic editor offers it: not hidden, not read-only, and
    /// still settable once the node runs.
    pub fn editable(&self) -> bool {
        self.flags & (Parameter::HIDDEN | Parameter::READ_ONLY) == 0 && self.rate != "ir"
    }

    /// **The range it is drawn over**, `(min, max, exponential)`. A declared
    /// bound wins; a control that declares none and goes by a conventional
    /// name (`freq`, `amp`, `pan`, ...) gets that name's range; any other is
    /// drawn over its default plus and minus the larger of the default's size
    /// and one. The default always lies inside.
    pub fn range(&self) -> (f64, f64, bool) {
        let declared = self.display.warp.as_deref().map(|warp| warp == "exp");
        if let (Some(lo), Some(hi)) = (self.display.min, self.display.max) {
            return (lo, hi, declared.unwrap_or(false) && lo > 0.0);
        }
        let (lo, hi, exp) = conventional(&self.name).map_or_else(
            || {
                let width = self.default.abs().max(1.0);
                (self.default - width, self.default + width, false)
            },
            |n| (n.min.min(self.default), n.max.max(self.default), n.exp),
        );
        (lo, hi, declared.unwrap_or(exp) && lo > 0.0)
    }

    /// The grid it moves on: the declared step, else a named range's.
    pub fn step(&self) -> f64 {
        self.display
            .step
            .or_else(|| conventional(&self.name).map(|n| n.step))
            .unwrap_or(0.0)
    }

    /// What its value is measured in: the declared unit, else a named range's.
    pub fn unit(&self) -> &str {
        self.display
            .unit
            .as_deref()
            .or_else(|| conventional(&self.name).map(|n| n.unit))
            .unwrap_or("")
    }

    /// Whether it matches a search: `query` within its name, its label or its
    /// group, ignoring case. An empty query matches everything.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [self.name.as_str(), self.label(), self.group.as_str()]
                .iter()
                .any(|field| field.to_lowercase().contains(&query))
    }
}

/// A conventional name's range.
struct Named {
    min: f64,
    max: f64,
    exp: bool,
    step: f64,
    unit: &'static str,
}

/// **The ranges a control's name conventionally means**, for a control whose
/// def declares none. The table is the field's long-standing one for synthesis
/// controls; a name not in it gets the generic rule of [`Control::range`].
fn conventional(name: &str) -> Option<Named> {
    let named = |min, max, exp, step, unit| {
        Some(Named {
            min,
            max,
            exp,
            step,
            unit,
        })
    };
    match name.to_ascii_lowercase().as_str() {
        "freq" | "frequency" | "cutoff" | "ffreq" => named(20.0, 20000.0, true, 0.0, "Hz"),
        "lofreq" => named(0.1, 100.0, true, 0.0, "Hz"),
        "midfreq" => named(25.0, 4200.0, true, 0.0, "Hz"),
        "widefreq" => named(0.1, 20000.0, true, 0.0, "Hz"),
        "amp" | "amplitude" | "level" => named(0.0, 1.0, false, 0.0, ""),
        "pan" | "balance" | "bipolar" => named(-1.0, 1.0, false, 0.0, ""),
        "unipolar" | "mix" => named(0.0, 1.0, false, 0.0, ""),
        "detune" => named(-20.0, 20.0, false, 0.0, "Hz"),
        "rate" => named(0.125, 8.0, true, 0.0, ""),
        "beats" => named(0.0, 20.0, false, 0.0, "Hz"),
        "delay" => named(0.0001, 1.0, true, 0.0, "s"),
        "rq" => named(0.001, 2.0, true, 0.0, ""),
        "phase" => named(0.0, std::f64::consts::TAU, false, 0.0, ""),
        "midinote" | "note" | "velocity" | "vel" => named(0.0, 127.0, false, 1.0, ""),
        "gate" => named(0.0, 1.0, false, 1.0, ""),
        _ => None,
    }
}

/// **A value as text**: four significant figures at most (fewer decimals the
/// larger the number), trailing zeros dropped, then the unit after a space.
pub fn format_value(value: f64, unit: &str) -> String {
    let decimals = match value.abs() {
        v if v >= 1000.0 => 0,
        v if v >= 100.0 => 1,
        v if v >= 10.0 => 2,
        _ => 3,
    };
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.truncate(text.trim_end_matches('0').trim_end_matches('.').len());
    }
    if text == "-0" {
        text = "0".into();
    }
    if unit.is_empty() {
        text
    } else {
        format!("{text} {unit}")
    }
}

/// **Text as a value**: a number, with the unit after it or not (in any
/// case). `None` for anything else, a number that is not finite included.
pub fn parse_value(text: &str, unit: &str) -> Option<f64> {
    let mut text = text.trim();
    if !unit.is_empty()
        && text.len() >= unit.len()
        && text[text.len() - unit.len()..].eq_ignore_ascii_case(unit)
    {
        text = text[..text.len() - unit.len()].trim_end();
    }
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// How many controls one page of the editor shows.
pub const PAGE: usize = 16;

/// The most steps a control with detents is a slider with: past it the
/// steps are too close to drag between, and it is a `number` with arrows.
const MAX_DETENTS: f64 = 128.0;

/// The height a row of the editor is laid out at, in logical pixels: what a
/// window of a few controls is sized by.
const ROW_H: f64 = 44.0;

/// **What the editor shows**: the window for `node`, whose def is called
/// `name` and has `controls`, with the controls that match `query`, on `page`
/// (clamped to the last). See [`Editor`] for what comes back.
pub fn editor(node: i32, name: &str, controls: &[Control], query: &str, page: usize) -> Editor {
    let mut shown: Vec<&Control> = controls
        .iter()
        .filter(|c| c.editable() && c.matches(query))
        .collect();
    // The groups in the order they first appear, each control in its own
    // order within its group: a group is read together, wherever its
    // controls were declared.
    let mut groups: Vec<&str> = Vec::new();
    for c in &shown {
        if !groups.contains(&c.group.as_str()) {
            groups.push(&c.group);
        }
    }
    shown.sort_by_key(|c| groups.iter().position(|g| *g == c.group));
    let total = shown.len();
    let count = total.div_ceil(PAGE).max(1);
    let page = page.min(count - 1);
    let pages = (0..count)
        .map(|n| {
            let first = n * PAGE;
            let last = ((n + 1) * PAGE).min(total);
            format!("{}-{last} of {total}", (first + 1).min(last))
        })
        .collect();
    let mut rows = Vec::new();
    let mut group = None;
    for c in shown.iter().skip(page * PAGE).take(PAGE) {
        if group != Some(c.group.as_str()) && !c.group.is_empty() {
            rows.push(json!({"type": "label", "text": c.group}));
        }
        group = Some(c.group.as_str());
        rows.push(row(node, c));
    }
    if rows.is_empty() {
        rows.push(json!({"type": "label", "text": "No control matches."}));
    }
    // Packed: each row as tall as what it holds, the rest of the window
    // under them, however few the controls.
    let contents = json!({
        "type": "layout",
        "name": "controls",
        "flow": "col",
        "pack": true,
        "weight": 1,
        "children": rows,
    });
    Editor {
        page,
        pages,
        contents,
        title: format!("{name} ({node})"),
    }
}

/// **The editor of one node, generated**: [`window`](Self::window) is the
/// whole of it, to open; when the search or the page changes, a client asks
/// again and redefines the widget named `controls` with
/// [`contents`](Self::contents), and sets the `page` choice to
/// [`pages`](Self::pages) and [`page`](Self::page).
pub struct Editor {
    /// The page shown, after clamping.
    pub page: usize,
    /// What each page is called (`"17-32 of 40"`), one at least.
    pub pages: Vec<String>,
    /// The rows of the page: the `layout` named `controls`.
    pub contents: Value,
    /// The window's title.
    pub title: String,
}

impl Editor {
    /// The window: a search field (`search`) and the page chooser (`page`)
    /// above the rows.
    pub fn window(&self, query: &str) -> Value {
        // As tall as the first page it opens on, up to a page of sixteen: a
        // row is a slider's height, and the search row one more.
        let rows = self.contents["children"].as_array().map_or(1, Vec::len);
        let h = (ROW_H * (rows + 1) as f64 + 24.0).min(640.0);
        json!({
            "type": "window",
            "title": self.title,
            "w": 560,
            "h": h,
            "children": [
                {
                    "type": "layout",
                    "flow": "row",
                    "hug": 1,
                    "pack": true,
                    "children": [
                        {"type": "label", "text": "Search"},
                        {"type": "text", "name": "search", "value": query, "weight": 1},
                        {"type": "choice", "name": "page", "options": self.pages,
                         "index": self.page, "weight": 1},
                    ],
                },
                self.contents,
            ],
        })
    }

    /// The answer the clients' door returns: `{"window", "contents", "pages",
    /// "page"}`.
    pub fn to_json(&self, query: &str) -> Value {
        json!({
            "window": self.window(query),
            "contents": self.contents,
            "pages": self.pages,
            "page": self.page,
        })
    }
}

/// One control's row: what it is called, the widget that sets it and its
/// value as text -- a label bound to `/node_setText`, which writes nothing
/// (a label reports no value) and is read back like any binding. The widget
/// is named `control:<name>` and the text `text:<name>`, so a script finds
/// either by the control; the editor's own names (`search`, `page`,
/// `controls`) hold no colon and so are never a control's.
fn row(node: i32, c: &Control) -> Value {
    let (min, max, exp) = c.range();
    let step = c.step();
    let set = json!(["/node_set", node, c.name]);
    let lists = !c.labels.is_empty() && step > 0.0;
    let toggles = step > 0.0 && ((max - min) / step - 1.0).abs() < 1e-9;
    let widget = if lists {
        let values: Vec<f64> = (0..c.labels.len()).map(|i| min + i as f64 * step).collect();
        let index = ((c.default - min) / step).round().max(0.0) as usize;
        json!({"type": "choice", "options": c.labels, "values": values,
               "index": index.min(c.labels.len() - 1), "bind": set, "weight": 2})
    } else if toggles {
        json!({"type": "toggle", "on": max, "off": min, "value": c.default >= max,
               "bind": set, "weight": 2})
    } else if step > 0.0 && (max - min) / step > MAX_DETENTS {
        // More steps than a groove has room for -- a bus number, a count: a
        // number moved a step at a time.
        json!({"type": "number", "min": min, "max": max, "step": step,
               "value": c.default, "stepper": true, "bind": set, "weight": 2})
    } else {
        // Its number is the text beside it, which is the server's.
        let mut slider = json!({"type": "slider", "min": min, "max": max, "step": step,
                                "value": c.default, "readout": false, "bind": set,
                                "weight": 2});
        if exp {
            slider["warp"] = json!("exp");
        }
        slider
    };
    let mut widget = widget;
    widget["name"] = json!(format!("control:{}", c.name));
    let text = if lists {
        let at = ((c.default - min) / step).round().max(0.0) as usize;
        c.labels.get(at).cloned().unwrap_or_default()
    } else {
        format_value(c.default, c.unit())
    };
    json!({
        "type": "layout",
        "flow": "row",
        "children": [
            {"type": "label", "text": c.label(), "weight": 1},
            widget,
            {"type": "label", "text": text, "weight": 1, "name": format!("text:{}", c.name),
             "bind": ["/node_setText", node, c.name]},
        ],
    })
}

/// **The door both clients call**: `{"node", "name", "controls", "query",
/// "page"}` in (the last two optional), [`Editor::to_json`] out, or
/// `{"error"}`.
pub fn editor_request(request: &Value) -> Value {
    let answer = || -> Result<Value, String> {
        let node = request["node"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or("the editor needs a node id")?;
        let name = request["name"].as_str().unwrap_or("");
        let controls: Vec<Control> = serde_json::from_value(request["controls"].clone())
            .map_err(|e| format!("a control description: {e}"))?;
        let query = request["query"].as_str().unwrap_or("");
        let page = request["page"].as_u64().unwrap_or(0) as usize;
        Ok(editor(node, name, &controls, query, page).to_json(query))
    };
    answer().unwrap_or_else(|e| json!({ "error": e }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(count: usize) -> Vec<Control> {
        (0..count)
            .map(|i| Control {
                label: Some(format!("Control {i}")),
                group: format!("Module {}", i / 100),
                display: ControlDisplay {
                    min: Some(0.0),
                    max: Some(1.0),
                    ..Default::default()
                },
                ..Control::new(format!("param:{i}"), 0.5)
            })
            .collect()
    }

    fn rows(editor: &Editor) -> &Vec<Value> {
        editor.contents["children"].as_array().unwrap()
    }

    /// **A def of thousands of controls draws one page**, and a search
    /// reaches the last of them.
    #[test]
    fn a_large_def_is_paged_and_searched() {
        let controls = plugin(2295);
        let first = editor(42, "big", &controls, "", 0);
        assert_eq!(first.pages.len(), 2295usize.div_ceil(PAGE));
        assert_eq!(first.pages[0], "1-16 of 2295");
        // one group heading and the page's controls
        assert_eq!(rows(&first).len(), PAGE + 1);
        let last = editor(42, "big", &controls, "control 2294", 9999);
        assert_eq!(last.page, 0);
        let text = last.contents.to_string();
        assert!(text.contains("param:2294"), "{text}");
        assert!(!text.contains("param:2293"));
    }

    /// **Each kind of control becomes its widget**, bound straight to the
    /// node, and what no editor offers is left out.
    #[test]
    fn each_control_becomes_its_widget() {
        let mut controls = plugin(6);
        controls[1].display.step = Some(1.0);
        controls[2].display.max = Some(2.0);
        controls[2].display.step = Some(1.0);
        controls[2].labels = vec!["Room".into(), "Hall".into(), "Plate".into()];
        controls[2].default = 0.0;
        controls[3].flags = Parameter::HIDDEN;
        controls[4].flags = Parameter::READ_ONLY;
        controls[5].rate = "ir".into();
        controls.push(Control {
            display: ControlDisplay {
                min: Some(-1.0),
                max: Some(1023.0),
                step: Some(1.0),
                ..Default::default()
            },
            group: "Module 0".into(),
            ..Control::new("out", 0.0)
        });
        let ed = editor(42, "fx", &controls, "", 0);
        let rows = rows(&ed);
        assert_eq!(rows.len(), 5, "a heading and four controls");
        assert_eq!(
            rows[4]["children"][1]["type"], "number",
            "a bus is a number"
        );
        let widget = |row: usize| &rows[row]["children"][1];
        assert_eq!(widget(1)["type"], "slider");
        assert_eq!(widget(1)["bind"], json!(["/node_set", 42, "param:0"]));
        assert_eq!(widget(1)["name"], "control:param:0");
        assert_eq!(widget(2)["type"], "toggle");
        assert_eq!(widget(3)["type"], "choice");
        assert_eq!(widget(3)["values"], json!([0.0, 1.0, 2.0]));
        assert_eq!(rows[3]["children"][2]["text"], "Room");
        assert_eq!(
            rows[3]["children"][2]["bind"],
            json!(["/node_setText", 42, "param:2"])
        );
    }

    /// **A declared range wins over a name's**, a name's over the generic
    /// rule, and the default is always inside.
    #[test]
    fn a_range_is_declared_then_named_then_generic() {
        let mut c = Control::new("freq", 440.0);
        assert_eq!(c.range(), (20.0, 20000.0, true));
        assert_eq!(c.unit(), "Hz");
        c.display.min = Some(100.0);
        c.display.max = Some(1000.0);
        assert_eq!(c.range(), (100.0, 1000.0, false));
        c.display.warp = Some("exp".into());
        assert_eq!(c.range(), (100.0, 1000.0, true));
        let c = Control::new("unknown", -7.0);
        assert_eq!(c.range(), (-14.0, 0.0, false));
        let c = Control::new("unknown", 0.0);
        assert_eq!(c.range(), (-1.0, 1.0, false));
        let c = Control::new("amp", 3.0);
        assert_eq!(c.range(), (0.0, 3.0, false));
    }

    /// **A description no view can draw is refused** when the def is built.
    #[test]
    fn an_impossible_display_is_refused() {
        let bad = |display: ControlDisplay| display.validate().is_err();
        assert!(bad(ControlDisplay {
            min: Some(1.0),
            max: Some(1.0),
            ..Default::default()
        }));
        assert!(bad(ControlDisplay {
            min: Some(1.0),
            ..Default::default()
        }));
        assert!(bad(ControlDisplay {
            step: Some(-1.0),
            ..Default::default()
        }));
        assert!(bad(ControlDisplay {
            warp: Some("log".into()),
            ..Default::default()
        }));
        assert!(bad(ControlDisplay {
            min: Some(0.0),
            max: Some(1.0),
            warp: Some("exp".into()),
            ..Default::default()
        }));
        assert!(bad(ControlDisplay {
            min: Some(0.0),
            max: Some(f64::NAN),
            ..Default::default()
        }));
        assert!(
            ControlDisplay {
                min: Some(20.0),
                max: Some(20000.0),
                warp: Some("exp".into()),
                unit: Some("Hz".into()),
                step: None,
            }
            .validate()
            .is_ok()
        );
    }

    /// **A value reads as text and the text reads back**, with or without
    /// its unit.
    #[test]
    fn a_value_reads_as_text_and_back() {
        assert_eq!(format_value(440.0, "Hz"), "440 Hz");
        assert_eq!(format_value(0.5, ""), "0.5");
        assert_eq!(format_value(0.123456, ""), "0.123");
        assert_eq!(format_value(12345.6, ""), "12346");
        assert_eq!(format_value(-0.0001, ""), "0");
        assert_eq!(parse_value(" 880 hz ", "Hz"), Some(880.0));
        assert_eq!(parse_value("0.25", "Hz"), Some(0.25));
        assert_eq!(parse_value("loud", ""), None);
        assert_eq!(parse_value("inf", ""), None);
    }

    /// **The clients' door** reads a request and answers the editor.
    #[test]
    fn the_door_answers_an_editor() {
        let request = json!({
            "node": 7, "name": "sine",
            "controls": [{"name": "freq", "default": 440.0, "rate": "kr"}],
        });
        let answer = editor_request(&request);
        assert_eq!(answer["window"]["title"], "sine (7)");
        assert_eq!(answer["pages"], json!(["1-1 of 1"]));
        assert!(editor_request(&json!({"name": "x"}))["error"].is_string());
    }
}
