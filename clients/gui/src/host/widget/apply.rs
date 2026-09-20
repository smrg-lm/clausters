//! Applying a `/gui_set` key/value to a live [`WidgetKind`] -- the incremental
//! wire-to-schema update, one arm per widget type. Split out of the schema
//! ([`super`]) alongside [`super::build`] so the enum reads separately from the
//! two long wire matches; the shared setter helpers live in [`super::parse`].

use serde_json::Value;

use super::*;

/// Applies one `/gui_set` key/value to `widget` -- its kind's own keys.
pub(super) fn apply_widget(widget: &mut Widget, key: &str, v: &Value) -> bool {
    apply_kind(&mut widget.kind, key, v)
}

/// Applies one `/gui_set` key/value to `kind`, returning whether the key was one
/// this widget accepts (and thus changed it).
pub(super) fn apply_kind(kind: &mut WidgetKind, key: &str, v: &Value) -> bool {
    match kind {
        // The status bar is the window's alone -- a panel has no bottom edge of
        // its own to talk on -- so it is answered before the arm the two share.
        WidgetKind::Window { status, .. } if key == "status" => {
            truthy(v).map(|b| *status = b).is_some()
        }
        WidgetKind::Window {
            layout, flow, hug, ..
        }
        | WidgetKind::Panel { layout, flow, hug } => match key {
            "flow" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            "hug" => truthy(v).map(|b| *hug = b).is_some(),
            _ => flow.apply(key, v),
        },
        // The page shown, live: this is the prop a bound toggle or menu drives.
        // A non-number leaves it alone rather than blanking the stack.
        WidgetKind::Stack { index, margin, hug } => match key {
            "index" => v.as_i64().map(|n| *index = n as i32).is_some(),
            "margin" => {
                *margin = v.as_f64().map(|n| n as f32);
                true
            }
            "hug" => truthy(v).map(|b| *hug = b).is_some(),
            _ => false,
        },
        WidgetKind::Scroll { layout, flow, view } => match key {
            "flow" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            _ => view.apply(key, v) || flow.apply(key, v),
        },
        // A free-standing ruler is its editor chrome and nothing else: the
        // unit it labels (`ruler`), the rate and the beat grid, the link that
        // joins it to the lanes. Without this arm a `/gui_set` of any of them
        // was recorded in the registry and never reached the drawing -- a
        // script could not change the unit of the strip it had just built.
        WidgetKind::TimeRuler { editor } => editor.apply(key, v),
        WidgetKind::Custom(el) => el.set(key, v),
        _ => false,
    }
}
