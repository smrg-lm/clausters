//! The migration's **expand** stage: the host reads the model's vocabulary and
//! rewrites it into the construction the current catalog names.
//!
//! The wire is moving from naming *widgets* to naming the **model** — a
//! container owning 0, 1 or 2 axes, and elements drawn against them. That is a
//! rename of everything at once, so it lands in three stages: the host learns
//! the new names first (here), both builders and every example switch to them
//! next, and this module is **deleted** last, when the constructions are named
//! by the new words directly instead of being reached through them. Nothing
//! outside this file knows the new vocabulary exists, which is exactly what
//! makes the last stage a deletion rather than an unwinding.
//!
//! Two things are rewritten, both back into what [`build`](super::build) and
//! [`apply`](super::apply) already read:
//!
//! - the **type name** — `layout`, `plane`, `field`, `signal`, `notes`,
//!   `curve`, `nodes`, `keys` — resolved to the catalog name whose props say
//!   the same thing;
//! - the **axis key** — `"axes": {"x": {…}, "y": {…}}` — flattened to the
//!   per-view chrome props (`view_start`, `ruler_y`, `y_len`, …) that carry it
//!   today. The nesting is what the model is *for*: the chrome belongs to the
//!   container's axes, not to each view that draws against them. Under an axis
//!   the name drops the axis marker, so `x.start` is `view_start` and `y.ruler`
//!   is `ruler_y`.
//!
//! The one thing expand cannot do is `box`, which the catalog already spends
//! on a synonym of `panel` while the model wants it for a patcher's box. The
//! collision is not resolvable while both spellings must parse, so in this
//! stage `box` keeps meaning the container and a plane's boxes stay the
//! `boxes` prop they are today.

use serde_json::{Map, Value};

use super::GuiNode;

/// The key an axis pair rides under. Not bare `x`/`y`: those are already the
/// free-placement props, and a container that is placed *and* owns axes would
/// have no way to say which it meant.
pub(crate) const AXES: &str = "axes";

/// The key a container's arrangement rides under — what the catalog spells
/// `layout`, which the model spends on the container type itself.
const FLOW: &str = "flow";

/// Rewrites a node written in the model's vocabulary into the one the catalog
/// reads. `None` — the common path — means the node needs no rewriting and its
/// own `type` and props are used as they are.
///
/// Only the type and the props are produced: the children are the caller's,
/// untouched, so a rewrite costs one props map per node rather than a clone of
/// the subtree under it.
pub(super) fn rewrite(node: &GuiNode) -> Option<(String, Map<String, Value>)> {
    let named = resolve(&node.kind, &node.props, !node.children.is_empty());
    let axes = node.props.get(AXES).and_then(Value::as_object);
    // `flow` is what the model calls a container's arrangement, on every
    // container that has one — so it is mapped here rather than in the arm of
    // whichever type happens to be resolving.
    let flow = node.props.contains_key(FLOW) && !node.props.contains_key("layout");
    if named.is_none() && axes.is_none() && !flow {
        return None;
    }
    let mut props = node.props.clone();
    props.remove(AXES);
    if let Some(arrangement) = flow.then(|| props[FLOW].clone()) {
        props.insert("layout".to_string(), arrangement);
    }
    if let Some(axes) = axes {
        flatten(axes, &mut props);
    }
    let (kind, extra) = named.unwrap_or_else(|| (node.kind.clone(), Vec::new()));
    for (key, value) in extra {
        props.insert(key.to_string(), value);
    }
    Some((kind, props))
}

/// Flattens every `axes` pair in a tree, in place — the pass a def makes on
/// the way in, before the registry records the node or the renderer reads it.
/// The node's `type` is untouched: only the chrome moves, so a `/gui_query`
/// still answers in the vocabulary the tree was written in.
pub(crate) fn flatten_tree(node: &mut GuiNode) {
    if let Some(Value::Object(axes)) = node.props.remove(AXES) {
        let mut flat = Map::new();
        flatten(&axes, &mut flat);
        for (key, value) in flat {
            node.props.entry(key).or_insert(value);
        }
    }
    for child in &mut node.children {
        flatten_tree(child);
    }
}

/// Flattens an `axes` pair into the props each axis is spelled as today,
/// without overwriting a flat prop the node also names — a node that says both
/// is mid-migration, and the spelling it is being migrated *from* is the one
/// its author most recently meant.
pub(crate) fn flatten(axes: &Map<String, Value>, out: &mut Map<String, Value>) {
    for (axis, table) in [("x", X_AXIS), ("y", Y_AXIS)] {
        let Some(keys) = axes.get(axis).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in keys {
            let Some((_, flat)) = table.iter().find(|(k, _)| k == key) else {
                tracing::debug!("axes.{axis}: no axis property {key:?}");
                continue;
            };
            out.entry(flat.to_string()).or_insert_with(|| value.clone());
        }
    }
}

/// The x axis' properties, and the prop each is spelled as today. `start` and
/// `len` are the axis' window; the rest already read as the axis' own.
const X_AXIS: &[(&str, &str)] = &[
    ("start", "view_start"),
    ("len", "view_len"),
    ("ruler", "ruler"),
    ("unit", "ruler"),
    ("tempo", "tempo"),
    ("beat_at", "beat_at"),
    ("quant", "quant"),
    ("sample_rate", "sample_rate"),
    ("link", "link"),
    ("sel_start", "sel_start"),
    ("sel_len", "sel_len"),
    ("playhead", "playhead"),
    ("playhead_at", "playhead_at"),
    ("playhead_loop_start", "playhead_loop_start"),
    ("playhead_loop_len", "playhead_loop_len"),
];

/// The y axis' properties. `min`/`max` are the value range five widgets carry
/// on themselves today; `ruler` is the `ruler_y` strip.
const Y_AXIS: &[(&str, &str)] = &[
    ("start", "y_start"),
    ("len", "y_len"),
    ("ruler", "ruler_y"),
    ("unit", "ruler_y"),
    ("min", "min"),
    ("max", "max"),
    ("bit_depth", "bit_depth"),
];

/// The catalog name a model name resolves to, with the props that resolution
/// implies — a `signal` whose view is a stored spectrum builds the `plot` that
/// reads `view: "spectrum"`, so the choice is recorded as a prop rather than
/// left for the reader of the tree to infer.
///
/// `has_children` distinguishes the two-axis containers that only differ by
/// whether anything is placed on them.
fn resolve(
    kind: &str,
    props: &Map<String, Value>,
    has_children: bool,
) -> Option<(String, Vec<(&'static str, Value)>)> {
    let named = |name: &str| Some((name.to_string(), Vec::new()));
    match kind {
        // A layout container arranges its children by `flow`, and `stack` — one
        // child at a time — is one of the arrangements rather than a type of
        // its own: a container with a selection instead of an arrangement.
        "layout" => {
            let flow = props
                .get(FLOW)
                .or_else(|| props.get("layout"))
                .and_then(Value::as_str);
            if flow == Some("stack") {
                named("stack")
            } else {
                named("panel")
            }
        }
        // Two axes locked to one scale. What the patcher adds to a plane is its
        // boxes and the wires between them, so it is the presence of those that
        // tells the two apart.
        "plane" => {
            if props.contains_key("boxes") || props.contains_key("cords") {
                named("patch")
            } else {
                named("scroll")
            }
        }
        // Two independent axes, told apart by what is on it: a placement makes
        // it a clip on its parent's x axis, a thickness with nothing placed and
        // no lane chrome makes it the free-standing ruler, and everything else
        // is a lane — including an empty one, which a multitrack opens all the
        // time and which must not read as a ruler.
        "field" => {
            let any = |keys: &[&str]| keys.iter().any(|k| props.contains_key(*k));
            if any(&["offset", "dur"]) {
                named("clip")
            } else if props.contains_key("h")
                && !has_children
                && !any(&[
                    "label", "height", "header_w", "mute", "solo", "level", "snap",
                ])
            {
                named("timeruler")
            } else {
                named("track")
            }
        }
        "signal" => Some(signal(props)),
        "notes" => named("pianoroll"),
        "curve" => named("bpf"),
        "nodes" => named("nodetree"),
        "keys" => named("piano"),
        _ => None,
    }
}

/// The signal element's catalog name: the point of the presentation × source
/// product the props describe. A source naming a `bus` is forward-only, and
/// everything else is addressable — which is the whole of what the six names
/// ever encoded, minus the capabilities, which are props of their own.
fn signal(props: &Map<String, Value>) -> (String, Vec<(&'static str, Value)>) {
    let live = props.contains_key("bus");
    let view = props.get("view").and_then(Value::as_str).unwrap_or("trace");
    // A trace over addressable samples that says it does not navigate is the
    // `plot` preset, not the `waveform` one with a capability turned off: the
    // two also differ in what they resolve their source as (a take through the
    // peak pyramid, or the sequence itself) and in whether an unnamed value
    // axis auto-fits, and neither is a capability a prop can flip afterwards.
    let still = props.get("navigable").and_then(super::truthy) == Some(false);
    let name = match (view, live) {
        ("spectrogram", _) => "spectrogram",
        ("phase", _) => "phasescope",
        ("spectrum", true) => "spectrum",
        // The stored spectrum is the `plot` that reads `view`, so the choice
        // rides as the prop that widget already parses.
        ("spectrum", false) => {
            return ("plot".to_string(), vec![("view", Value::from("spectrum"))]);
        }
        (_, true) => "scope",
        (_, false) if still => "plot",
        (_, false) => "waveform",
    };
    (name.to_string(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::super::{Layout, Widget, WidgetKind};
    use super::*;
    use crate::host::guidef::GuiNode;
    use crate::host::signal::Presentation;

    fn build(json: &str) -> WidgetKind {
        let node = GuiNode::parse(json.as_bytes()).expect("the test tree parses");
        Widget::from_node(1, &node, &[])
            .expect("the test tree builds")
            .kind
    }

    /// Every model name reaches a construction, and reaches the *same* one the
    /// catalog name reaches — which is what makes the last stage a deletion of
    /// this module rather than a second parse.
    #[test]
    fn a_model_name_builds_what_its_catalog_name_builds() {
        for (model, catalog) in [
            (
                r#"{"type":"layout","layout":"row"}"#,
                r#"{"type":"panel","layout":"row"}"#,
            ),
            (
                r#"{"type":"layout","flow":"stack","index":1}"#,
                r#"{"type":"stack","index":1}"#,
            ),
            (
                r#"{"type":"plane","axis":"y"}"#,
                r#"{"type":"scroll","axis":"y"}"#,
            ),
            (
                r#"{"type":"plane","boxes":[],"cords":[]}"#,
                r#"{"type":"patch","boxes":[],"cords":[]}"#,
            ),
            (
                r#"{"type":"field","offset":0.0,"dur":48000.0}"#,
                r#"{"type":"clip","offset":0.0,"dur":48000.0}"#,
            ),
            (
                r#"{"type":"field","h":24.0}"#,
                r#"{"type":"timeruler","h":24.0}"#,
            ),
            (
                r#"{"type":"notes","min":48,"max":84}"#,
                r#"{"type":"pianoroll","min":48,"max":84}"#,
            ),
            (
                r#"{"type":"curve","min":0.0,"max":1.0}"#,
                r#"{"type":"bpf","min":0.0,"max":1.0}"#,
            ),
            (
                r#"{"type":"nodes","group":0}"#,
                r#"{"type":"nodetree","group":0}"#,
            ),
            (
                r#"{"type":"keys","min":36,"max":96}"#,
                r#"{"type":"piano","min":36,"max":96}"#,
            ),
        ] {
            assert_eq!(
                format!("{:?}", build(model)),
                format!("{:?}", build(catalog)),
                "{model} and {catalog} name the same construction"
            );
        }
    }

    /// A `field` holding something is a lane; a bare strip of a given
    /// thickness is the free-standing ruler. One container, told apart by what
    /// is placed on it rather than by two type names — and an **empty lane**
    /// is a lane, since a multitrack opens those and a ruler is not what it
    /// wanted.
    #[test]
    fn a_field_is_a_lane_or_a_bare_ruler_by_what_is_on_it() {
        assert!(matches!(
            build(r#"{"type":"field","children":[{"type":"field","id":2,"dur":100.0}]}"#),
            WidgetKind::Track { .. }
        ));
        assert!(matches!(
            build(r#"{"type":"field","h":24.0}"#),
            WidgetKind::TimeRuler { .. }
        ));
        assert!(matches!(
            build(r#"{"type":"field"}"#),
            WidgetKind::Track { .. }
        ));
        assert!(matches!(
            build(r#"{"type":"field","h":24.0,"label":"drums"}"#),
            WidgetKind::Track { .. }
        ));
    }

    /// The six signal names were six points of one product, so the model says
    /// the point and the name falls out of it.
    #[test]
    fn a_signal_names_the_point_of_the_product_the_old_names_encoded() {
        let presentation = |json: &str| match build(json) {
            WidgetKind::Signal(el) => (el.presentation, el.is_live()),
            other => panic!("{json} did not build a signal element: {other:?}"),
        };
        assert_eq!(
            presentation(r#"{"type":"signal","data":[0.0,1.0]}"#),
            (Presentation::Signal, false)
        );
        assert_eq!(
            presentation(r#"{"type":"signal","bus":0}"#),
            (Presentation::Signal, true)
        );
        assert_eq!(
            presentation(r#"{"type":"signal","view":"spectrum","bus":0}"#),
            (Presentation::Spectrum, true)
        );
        assert_eq!(
            presentation(r#"{"type":"signal","view":"spectrum","data":[0.0,1.0]}"#),
            (Presentation::Spectrum, false)
        );
        assert_eq!(
            presentation(r#"{"type":"signal","view":"spectrogram","data":[0.0,1.0]}"#),
            (Presentation::TimeFrequency, false)
        );
        assert_eq!(
            presentation(r#"{"type":"signal","view":"phase","bus":0}"#),
            (Presentation::Phase, true)
        );
    }

    /// The capabilities stop being welded to the name: a trace that does not
    /// navigate is a prop away, where before it was a different widget.
    #[test]
    fn the_capabilities_are_props_rather_than_a_choice_of_name() {
        let caps = |json: &str| match build(json) {
            WidgetKind::Signal(el) => el.caps,
            other => panic!("not a signal element: {other:?}"),
        };
        assert!(caps(r#"{"type":"signal","data":[0.0]}"#).navigable);
        assert!(caps(r#"{"type":"signal","bus":0,"selectable":true}"#).selectable);
        // The one capability that is not only a capability: a trace that does
        // not navigate is the whole `plot` construction, source resolution and
        // auto-fitted value axis included.
        assert_eq!(
            format!(
                "{:?}",
                build(r#"{"type":"signal","data":[0.0],"navigable":false}"#)
            ),
            format!("{:?}", build(r#"{"type":"plot","data":[0.0]}"#))
        );
    }

    /// The axis chrome nests under the container's axes and lands on the props
    /// that carry it today — which is the half of the migration that moves a
    /// prop rather than a name.
    #[test]
    fn an_axis_pair_flattens_onto_the_props_that_carry_it_today() {
        let node = GuiNode::parse(
            br#"{"type":"field","h":24.0,
                 "axes":{"x":{"ruler":"beats","tempo":2.0,"start":100.0,"len":900.0},
                         "y":{"ruler":"db","min":-1.0,"max":1.0}}}"#,
        )
        .unwrap();
        let (kind, props) = rewrite(&node).expect("the axes key is rewritten");
        assert_eq!(kind, "timeruler");
        assert!(!props.contains_key(AXES), "the key itself is consumed");
        assert_eq!(props.get("ruler").and_then(Value::as_str), Some("beats"));
        assert_eq!(props.get("view_start").and_then(Value::as_f64), Some(100.0));
        assert_eq!(props.get("view_len").and_then(Value::as_f64), Some(900.0));
        assert_eq!(props.get("ruler_y").and_then(Value::as_str), Some("db"));
        assert_eq!(props.get("min").and_then(Value::as_f64), Some(-1.0));
    }

    /// A node still spelled the old way is not rewritten at all — the alias
    /// layer is a door the new vocabulary walks through, not a pass over every
    /// tree the host parses.
    #[test]
    fn a_node_the_catalog_already_names_is_left_alone() {
        let node = GuiNode::parse(br#"{"type":"panel","layout":"row"}"#).unwrap();
        assert!(rewrite(&node).is_none());
    }

    /// A flat prop beside an axis pair wins: the tree is mid-migration and the
    /// old spelling is the one its author last touched.
    #[test]
    fn a_flat_prop_beside_an_axis_pair_keeps_its_value() {
        let node = GuiNode::parse(
            br#"{"type":"field","h":24.0,"view_start":7.0,"axes":{"x":{"start":9.0}}}"#,
        )
        .unwrap();
        let (_, props) = rewrite(&node).unwrap();
        assert_eq!(props.get("view_start").and_then(Value::as_f64), Some(7.0));
    }

    /// `flow` is the arrangement on every container that has one — the model
    /// spends `layout` on the container type itself, so a window and a plane
    /// read it too, not only the container named after it.
    #[test]
    fn a_containers_flow_is_the_arrangement_it_gets() {
        assert!(matches!(
            build(r#"{"type":"layout","flow":"grid","cols":3}"#),
            WidgetKind::Panel {
                layout: Layout::Grid,
                ..
            }
        ));
        assert!(matches!(
            build(r#"{"type":"window","flow":"row"}"#),
            WidgetKind::Window {
                layout: Layout::Row,
                ..
            }
        ));
        assert!(matches!(
            build(r#"{"type":"plane","flow":"col"}"#),
            WidgetKind::Scroll {
                layout: Layout::Col,
                ..
            }
        ));
    }
}
