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

use std::sync::Arc;

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
    Waveform {
        samples: Arc<[f32]>,
        base_bucket: usize,
    },
    /// A type this build does not render yet (a newer or control widget). Laid
    /// out so it reserves space, but not painted. Carries the type tag for logs.
    Unknown(String),
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
            },
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
}

/// A non-negative integer dimension property, defaulted when absent.
fn dimension(props: &serde_json::Map<String, Value>, key: &str, default: u32) -> u32 {
    props
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, u32::MAX as u64) as u32)
        .unwrap_or(default)
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
    if props.contains_key("buffer") {
        // The server-buffer path needs the host's audio-server client leg, which
        // a later milestone adds. Render empty until then rather than fail.
        return Ok(Arc::from([] as [f32; 0]));
    }
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
            } => {
                assert_eq!(&samples[..], &[0.0, 0.5, -0.5, 1.0]);
                assert_eq!(*base_bucket, 2);
            }
            other => panic!("expected waveform, got {other:?}"),
        }
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
        let n = node(r#"{"type":"window","children":[{"id":7,"type":"knob"}]}"#);
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
            WidgetKind::Unknown(t) => assert_eq!(t, "knob"),
            other => panic!("expected unknown, got {other:?}"),
        }
    }

    #[test]
    fn bad_blob_index_is_an_error() {
        let n = node(r#"{"type":"window","children":[{"id":2,"type":"waveform","blob":3}]}"#);
        assert!(Widget::from_node(1, &n, &[]).is_err());
    }
}
