//! The typed widget schema: a renderer's interpretation of a GuiDef tree.
//!
//! `host::guidef::GuiNode` is the **generic** wire form (any `{id, type, props,
//! children}`), kept deliberately open so the protocol never changes when a
//! widget type is added. This module is the other half of that principle: the
//! *renderer* turns a `GuiNode` into a **typed** [`Widget`] it knows how to lay
//! out and draw. Adding a widget type is a new [`WidgetKind`] variant plus a
//! handler here and in the renderer — not a protocol change. An unrecognized
//! type is not an error: it becomes [`WidgetKind::Unknown`], laid out (it
//! reserves its space) but not painted, so a host built today renders the parts
//! of a newer GuiDef it understands and ignores the rest.
//!
//! The standardized widgets at this milestone are `window` + `panel`/layout
//! (`row`/`col`/`grid`/`free`) + `label`, plus the heavy `waveform` view, fed
//! its samples either inline (`"data": [f32…]`) or — for bulk — from an OSC blob
//! carried alongside the JSON in the same `/gui_def` message (`"blob": <index>`).
//! Both keep the int/float distinction and the "flat primitives at the boundary"
//! rule; a server buffer reference (`"buffer"`) is recognized but deferred to the
//! milestone where the host attaches to the audio server.

use std::path::PathBuf;
use std::sync::Arc;

use clausters_core::osc::OscType;
use serde_json::Value;

use super::guidef::GuiNode;

/// How a container arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Row,
    Col,
    Grid,
    Free,
}

impl Layout {
    /// Parses the `layout` property; defaults to `Col`.
    fn parse(props: &serde_json::Map<String, Value>) -> Layout {
        match props.get("layout").and_then(Value::as_str) {
            Some("row") => Layout::Row,
            Some("grid") => Layout::Grid,
            Some("free") => Layout::Free,
            _ => Layout::Col,
        }
    }
}

/// The typed kind of a widget, with the fields the renderer needs.
#[derive(Debug, Clone)]
pub enum WidgetKind {
    /// A top-level window (a GuiDef root): title, requested size, child layout.
    Window {
        title: Option<String>,
        width: u32,
        height: u32,
        layout: Layout,
    },
    /// A nestable container.
    Panel { layout: Layout },
    /// Static text.
    Label { text: String },
    /// The heavy waveform view: its samples and the peak-pyramid bucket size.
    /// The samples reach the view one of several ways, in precedence order:
    /// `cache` (a prebuilt peak-pyramid file the host maps — the most compact
    /// bulk path, raw samples never loaded), `path` (a file of raw little-endian
    /// `f32` the host maps — the bulk path for a multi-megabyte buffer, no OSC),
    /// `buffer` (an audio-server buffer number the windowed front fetches over
    /// the client leg), or inline `data`/`blob`. `channels` de-interleaves
    /// channel 0 of a multi-channel `path` (default 1). For `cache`/`path`/
    /// `buffer`, `samples` starts empty and is filled when the resource is
    /// mapped/fetched.
    Waveform {
        samples: Arc<[f32]>,
        base_bucket: usize,
        buffer: Option<i32>,
        path: Option<PathBuf>,
        cache: Option<PathBuf>,
        channels: usize,
    },
    /// A level meter reading control bus `bus` from the shared-memory segment
    /// each frame (zero messages), shown as a bar over `[min, max]`.
    Meter {
        bus: i32,
        min: f32,
        max: f32,
        label: Option<String>,
    },
    /// A time-domain scope plotting the recent history of control bus `bus`
    /// (read from shared memory each frame) over `[min, max]`.
    Scope {
        bus: i32,
        min: f32,
        max: f32,
        label: Option<String>,
    },
    /// A continuous slider over `[min, max]`.
    Slider(Range),
    /// A rotary control over `[min, max]`.
    Knob(Range),
    /// A draggable numeric read-out over `[min, max]`.
    Number(Range),
    /// A momentary push button.
    Button { label: Option<String> },
    /// A boolean on/off control.
    Toggle { value: bool, label: Option<String> },
    /// A free-text field showing its value (script-driven at this milestone).
    Text {
        value: String,
        label: Option<String>,
    },
    /// A drop/cycle selector over `options`, holding the chosen index.
    Menu {
        index: usize,
        options: Vec<String>,
        label: Option<String>,
    },
    /// A type this build does not render yet. Laid out so it reserves space, but
    /// not painted. Carries the type tag for logs.
    Unknown(String),
}

/// The shared payload of the continuous controls (`slider`/`knob`/`number`): a
/// value clamped to a range, with an optional label.
#[derive(Debug, Clone)]
pub struct Range {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub label: Option<String>,
}

impl Range {
    fn parse(props: &serde_json::Map<String, Value>) -> Range {
        let min = number(props, "min", 0.0);
        let max = number(props, "max", 1.0);
        let value = number(props, "value", min).clamp(min.min(max), min.max(max));
        Range {
            value,
            min,
            max,
            label: label(props),
        }
    }

    /// The value as a 0..1 fraction of the range (for rendering).
    pub fn fraction(&self) -> f32 {
        if (self.max - self.min).abs() < f32::EPSILON {
            0.0
        } else {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    /// Sets the value from a 0..1 fraction of the range (for interaction).
    pub fn set_fraction(&mut self, t: f32) {
        self.value = self.min + t.clamp(0.0, 1.0) * (self.max - self.min);
    }
}

/// The default window size when a GuiDef omits `w`/`h`.
const DEFAULT_WINDOW: (u32, u32) = (640, 360);
/// The default peak-pyramid bucket for an inline waveform.
const DEFAULT_BASE_BUCKET: usize = 256;

/// A typed widget node: its id (the root's comes from the `/gui_def` argument),
/// its kind, and its children (only containers have any).
#[derive(Debug, Clone)]
pub struct Widget {
    pub id: Option<i32>,
    pub kind: WidgetKind,
    pub children: Vec<Widget>,
}

impl Widget {
    /// Interprets a generic [`GuiNode`] (and the blobs carried beside it in the
    /// `/gui_def` message) into a typed widget tree. `root_id` is the def id from
    /// the OSC argument, used for the root whose JSON carries no `id`.
    pub fn from_node(root_id: i32, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        Self::build(Some(root_id), node, blobs)
    }

    fn build(id: Option<i32>, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        let id = id.or(node.id);
        let kind = match node.kind.as_str() {
            "window" => WidgetKind::Window {
                title: node
                    .props
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                width: dimension(&node.props, "w", DEFAULT_WINDOW.0),
                height: dimension(&node.props, "h", DEFAULT_WINDOW.1),
                layout: Layout::parse(&node.props),
            },
            "panel" | "box" => WidgetKind::Panel {
                layout: Layout::parse(&node.props),
            },
            "label" => WidgetKind::Label {
                text: node
                    .props
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "waveform" => WidgetKind::Waveform {
                samples: waveform_samples(id, &node.props, blobs)?,
                base_bucket: node
                    .props
                    .get("base_bucket")
                    .and_then(Value::as_u64)
                    .map(|n| (n as usize).max(1))
                    .unwrap_or(DEFAULT_BASE_BUCKET),
                buffer: node
                    .props
                    .get("buffer")
                    .and_then(Value::as_i64)
                    .map(|n| n as i32),
                path: node
                    .props
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
                cache: node
                    .props
                    .get("cache")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
                channels: node
                    .props
                    .get("channels")
                    .and_then(Value::as_u64)
                    .map(|n| (n as usize).max(1))
                    .unwrap_or(1),
            },
            "meter" => WidgetKind::Meter {
                bus: int_prop(&node.props, "bus", 0),
                min: number(&node.props, "min", 0.0),
                max: number(&node.props, "max", 1.0),
                label: label(&node.props),
            },
            "scope" => WidgetKind::Scope {
                bus: int_prop(&node.props, "bus", 0),
                min: number(&node.props, "min", -1.0),
                max: number(&node.props, "max", 1.0),
                label: label(&node.props),
            },
            "slider" => WidgetKind::Slider(Range::parse(&node.props)),
            "knob" => WidgetKind::Knob(Range::parse(&node.props)),
            "number" => WidgetKind::Number(Range::parse(&node.props)),
            "button" => WidgetKind::Button {
                label: label(&node.props),
            },
            "toggle" => WidgetKind::Toggle {
                value: node.props.get("value").and_then(truthy).unwrap_or(false),
                label: label(&node.props),
            },
            "text" => WidgetKind::Text {
                value: node
                    .props
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                label: label(&node.props),
            },
            "menu" => {
                let options = options(&node.props);
                let index = node.props.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                WidgetKind::Menu {
                    index: index.min(options.len().saturating_sub(1)),
                    options,
                    label: label(&node.props),
                }
            }
            other => WidgetKind::Unknown(other.to_string()),
        };
        // Only containers carry children into the typed tree; a leaf's children
        // (if any) are ignored.
        let children = match kind {
            WidgetKind::Window { .. } | WidgetKind::Panel { .. } => node
                .children
                .iter()
                .map(|c| Self::build(None, c, blobs))
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok(Widget { id, kind, children })
    }

    /// Whether this is the heavy waveform view (a convenience for the renderer).
    pub fn is_waveform(&self) -> bool {
        matches!(self.kind, WidgetKind::Waveform { .. })
    }

    /// The widget with id `id` anywhere in this tree, mutably (for `/gui_set`
    /// and interaction).
    pub fn find_mut(&mut self, id: i32) -> Option<&mut Widget> {
        if self.id == Some(id) {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }
}

impl WidgetKind {
    /// The current value as an OSC primitive for a `/gui_event`, or `None` for a
    /// non-interactive widget. A `button` reports `1` (it is momentary; the press
    /// is the event).
    pub fn event_value(&self) -> Option<OscType> {
        match self {
            WidgetKind::Slider(r) | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                Some(OscType::Float(r.value))
            }
            WidgetKind::Toggle { value, .. } => Some(OscType::Int(*value as i32)),
            WidgetKind::Menu { index, .. } => Some(OscType::Int(*index as i32)),
            WidgetKind::Text { value, .. } => Some(OscType::String(value.clone())),
            WidgetKind::Button { .. } => Some(OscType::Int(1)),
            _ => None,
        }
    }

    /// The control bus a live (shared-memory-backed) widget reads each frame, if
    /// this is one. The windowed front uses it to know which windows to animate
    /// and which bus to sample.
    pub fn live_bus(&self) -> Option<i32> {
        match self {
            WidgetKind::Meter { bus, .. } | WidgetKind::Scope { bus, .. } => Some(*bus),
            _ => None,
        }
    }

    /// Applies one `/gui_set` key/value to a live widget, returning whether it
    /// changed anything the renderer cares about.
    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        match self {
            WidgetKind::Meter {
                bus,
                min,
                max,
                label,
            }
            | WidgetKind::Scope {
                bus,
                min,
                max,
                label,
            } => match key {
                "bus" => v.as_i64().map(|n| *bus = n as i32).is_some(),
                "min" => set_f(min, v),
                "max" => set_f(max, v),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Slider(r) | WidgetKind::Knob(r) | WidgetKind::Number(r) => match key {
                "value" => set_f(&mut r.value, v),
                "min" => set_f(&mut r.min, v),
                "max" => set_f(&mut r.max, v),
                "label" => set_label(&mut r.label, v),
                _ => false,
            },
            WidgetKind::Toggle { value, label } => match key {
                "value" => truthy(v).map(|b| *value = b).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Text { value, label } => match key {
                "value" => v.as_str().map(|s| *value = s.to_string()).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Menu {
                index,
                options,
                label,
            } => match key {
                "index" => v
                    .as_u64()
                    .map(|n| *index = (n as usize).min(options.len().saturating_sub(1)))
                    .is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Button { label } => key == "label" && set_label(label, v),
            WidgetKind::Label { text } => {
                key == "text" && v.as_str().map(|s| *text = s.to_string()).is_some()
            }
            _ => false,
        }
    }
}

/// A non-negative integer dimension property, defaulted when absent.
fn dimension(props: &serde_json::Map<String, Value>, key: &str, default: u32) -> u32 {
    props
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, u32::MAX as u64) as u32)
        .unwrap_or(default)
}

/// An integer property, defaulted when absent or non-integer.
fn int_prop(props: &serde_json::Map<String, Value>, key: &str, default: i32) -> i32 {
    props
        .get(key)
        .and_then(Value::as_i64)
        .map(|n| n as i32)
        .unwrap_or(default)
}

/// A float property, defaulted when absent or non-numeric.
fn number(props: &serde_json::Map<String, Value>, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(default)
}

/// The `label` property as an owned string, if present.
fn label(props: &serde_json::Map<String, Value>) -> Option<String> {
    props
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `options` property as a list of strings (for a menu).
fn options(props: &serde_json::Map<String, Value>) -> Vec<String> {
    match props.get("options") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// A JSON value as a boolean: real bool, or a number where non-zero is true.
fn truthy(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x != 0.0),
        _ => None,
    }
}

/// Sets `slot` from a numeric JSON value, reporting whether it applied.
fn set_f(slot: &mut f32, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = x as f32;
            true
        }
        None => false,
    }
}

/// Sets an optional label from a string JSON value.
fn set_label(slot: &mut Option<String>, v: &Value) -> bool {
    match v.as_str() {
        Some(s) => {
            *slot = Some(s.to_string());
            true
        }
        None => false,
    }
}

/// Resolves a waveform widget's samples: inline `"data": [f32…]`, or `"blob":
/// <index>` into the OSC blobs carried with the def (raw little-endian `f32`).
fn waveform_samples(
    id: Option<i32>,
    props: &serde_json::Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<Arc<[f32]>, String> {
    let label = id.map_or_else(|| "waveform".to_string(), |i| format!("waveform {i}"));
    if let Some(Value::Array(items)) = props.get("data") {
        let samples: Vec<f32> = items
            .iter()
            .map(|v| v.as_f64().map(|x| x as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| format!("{label}: `data` must be an array of numbers"))?;
        return Ok(samples.into());
    }
    if let Some(index) = props.get("blob").and_then(Value::as_u64) {
        let blob = blobs.get(index as usize).ok_or_else(|| {
            format!(
                "{label}: `blob` {index} out of range ({} sent)",
                blobs.len()
            )
        })?;
        if blob.len() % 4 != 0 {
            return Err(format!(
                "{label}: blob length {} is not a multiple of 4",
                blob.len()
            ));
        }
        let samples: Vec<f32> = blob
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        return Ok(samples.into());
    }
    // A `buffer` (audio-server fetch) or a `path`/`cache` (mapped local
    // resource) is loaded later by the windowed front; start empty.
    Ok(Arc::from([] as [f32; 0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(json: &str) -> GuiNode {
        GuiNode::parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn window_with_inline_waveform() {
        let n = node(
            r#"{"type":"window","title":"W","w":480,"h":240,"layout":"col",
                "children":[{"id":12,"type":"waveform","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        assert_eq!(w.id, Some(1));
        match w.kind {
            WidgetKind::Window {
                title,
                width,
                height,
                layout,
            } => {
                assert_eq!(title.as_deref(), Some("W"));
                assert_eq!((width, height), (480, 240));
                assert_eq!(layout, Layout::Col);
            }
            other => panic!("expected window, got {other:?}"),
        }
        assert_eq!(w.children.len(), 1);
        match &w.children[0].kind {
            WidgetKind::Waveform {
                samples,
                base_bucket,
                buffer,
                ..
            } => {
                assert_eq!(&samples[..], &[0.0, 0.5, -0.5, 1.0]);
                assert_eq!(*base_bucket, 2);
                assert_eq!(*buffer, None);
            }
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn waveform_by_server_buffer_starts_empty_with_the_buffer_number() {
        let n = node(r#"{"type":"window","children":[{"id":3,"type":"waveform","buffer":7}]}"#);
        let w = Widget::from_node(1, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform {
                samples, buffer, ..
            } => {
                assert!(samples.is_empty(), "no inline data yet — fetched later");
                assert_eq!(*buffer, Some(7));
            }
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn waveform_by_path_and_cache_defer_with_their_props() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"waveform","path":"/tmp/buf.f32","channels":2},
                {"id":2,"type":"waveform","cache":"/tmp/buf.peaks"}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform {
                samples,
                path,
                channels,
                ..
            } => {
                assert!(samples.is_empty(), "samples are mapped later, not inline");
                assert_eq!(path.as_deref(), Some(std::path::Path::new("/tmp/buf.f32")));
                assert_eq!(*channels, 2);
            }
            other => panic!("expected waveform, got {other:?}"),
        }
        match &w.children[1].kind {
            WidgetKind::Waveform { cache, .. } => {
                assert_eq!(
                    cache.as_deref(),
                    Some(std::path::Path::new("/tmp/buf.peaks"))
                );
            }
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn meter_and_scope_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"meter","bus":5,"max":2.0,"label":"out"},
                {"id":2,"type":"scope","bus":6}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Meter {
                bus,
                min,
                max,
                label,
            } => {
                assert_eq!((*bus, *min, *max), (5, 0.0, 2.0));
                assert_eq!(label.as_deref(), Some("out"));
            }
            other => panic!("expected meter, got {other:?}"),
        }
        // The scope defaults to the bipolar [-1, 1] range.
        match &w.children[1].kind {
            WidgetKind::Scope { bus, min, max, .. } => {
                assert_eq!((*bus, *min, *max), (6, -1.0, 1.0))
            }
            other => panic!("expected scope, got {other:?}"),
        }
        assert_eq!(w.children[0].kind.live_bus(), Some(5));
        // A live `/gui_set` can retarget the bus and rescale the meter.
        let meter = w.find_mut(1).unwrap();
        assert!(meter.kind.apply("bus", &Value::from(8)));
        assert!(meter.kind.apply("max", &Value::from(4.0)));
        assert_eq!(meter.kind.live_bus(), Some(8));
    }

    #[test]
    fn waveform_from_blob() {
        let blob: Vec<u8> = [1.0f32, -1.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let n = node(r#"{"type":"window","children":[{"id":2,"type":"waveform","blob":0}]}"#);
        let w = Widget::from_node(1, &n, &[blob]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform { samples, .. } => assert_eq!(&samples[..], &[1.0, -1.0]),
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn defaults_and_unknown_type() {
        // `scope` is in the catalog but not yet a rendered WidgetKind variant.
        // `spectrum` is in the catalog but not yet a rendered WidgetKind variant.
        let n = node(r#"{"type":"window","children":[{"id":7,"type":"spectrum"}]}"#);
        let w = Widget::from_node(1, &n, &[]).unwrap();
        // Window size defaults when w/h are omitted.
        match w.kind {
            WidgetKind::Window {
                width,
                height,
                layout,
                ..
            } => {
                assert_eq!((width, height), DEFAULT_WINDOW);
                assert_eq!(layout, Layout::Col);
            }
            _ => unreachable!(),
        }
        // An unrecognized type is kept (laid out), not rejected.
        match &w.children[0].kind {
            WidgetKind::Unknown(t) => assert_eq!(t, "spectrum"),
            other => panic!("expected unknown, got {other:?}"),
        }
    }

    #[test]
    fn bad_blob_index_is_an_error() {
        let n = node(r#"{"type":"window","children":[{"id":2,"type":"waveform","blob":3}]}"#);
        assert!(Widget::from_node(1, &n, &[]).is_err());
    }

    #[test]
    fn parses_controls_and_clamps_value() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"slider","min":20.0,"max":2000.0,"value":5000.0,"label":"cut"},
                {"id":2,"type":"toggle","value":1},
                {"id":3,"type":"menu","options":["a","b","c"],"index":1}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Slider(r) => {
                assert_eq!(r.value, 2000.0, "value clamps into the range");
                assert_eq!(r.label.as_deref(), Some("cut"));
                assert_eq!(r.fraction(), 1.0);
            }
            other => panic!("expected slider, got {other:?}"),
        }
        assert!(matches!(
            w.children[1].kind,
            WidgetKind::Toggle { value: true, .. }
        ));
        assert!(matches!(
            &w.children[2].kind,
            WidgetKind::Menu { index: 1, .. }
        ));
    }

    #[test]
    fn apply_updates_value_and_event_value_reports_it() {
        let n =
            node(r#"{"type":"window","children":[{"id":5,"type":"knob","min":0.0,"max":10.0}]}"#);
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let knob = w.find_mut(5).unwrap();
        assert!(knob.kind.apply("value", &Value::from(4.0)));
        assert_eq!(knob.kind.event_value(), Some(OscType::Float(4.0)));
        // An unknown key is a no-op.
        assert!(!knob.kind.apply("nonesuch", &Value::from(1.0)));
    }
}
