//! **The built-in elements**: the leaves that have moved behind
//! [`Element`](super::widget::Element).
//!
//! A leaf used to be an arm in each of eight passes. Here it is one type
//! implementing one trait, in its own file, with its constructor named in the
//! table below — which is the whole of what the schema needs to know about it.
//! The enum keeps the **containers** (`window`, `layout`, `plane`, `field` and
//! the clip), because the layout pass has to know every coordinate system it
//! places into.
//!
//! **Why a table here and not the shared registry.** The order is the
//! invariant: a built-in name resolves *before* anything a program registered
//! ([`element::register`](super::widget::element::register)), so a registration
//! can never shadow a built-in or change what a shipped def means. Keeping the
//! two lookups separate is what states that, rather than relying on whoever
//! inserted first.
//!
//! It is also the seam the crate's features hang on: a family compiled out
//! drops its rows from this table, the lookup falls through to the registry and
//! then to `Unknown` — laid out, not painted — which is exactly how a host
//! older than the def already behaves. No new failure mode, and nothing for a
//! script to learn. `notation` (the `score` row) and `patcher` are the two that
//! use it today; both are on by default, since a feature is here so a build can
//! *drop* a family and never so it has to ask for one.

use serde_json::{Map, Value};

use super::widget::element::Constructor;
use super::widget::parse::truthy;

/// Whether a hand may edit **one body** of a clip, from the props the clip
/// carries.
///
/// Two keys answer it and the order is the point. `editable` is the statement
/// about the *clip* — it reaches every body the clip has, which is what the
/// `/gui_set` route already says of it — so a body that needs its own answer
/// reads `own` first: `notes_editable` for the roll, `points_editable` for the
/// curve. Without that split there is no way to say the one thing a rendered
/// generator needs said, which is that its notes cannot be written while the
/// envelope drawn over them can.
///
/// The default is editable, because a clip that says nothing offers what it
/// draws.
pub(crate) fn body_editable(props: &Map<String, Value>, own: &str) -> bool {
    props
        .get(own)
        .or_else(|| props.get("editable"))
        .and_then(truthy)
        .unwrap_or(true)
}

mod button;
mod canvas;
mod control;
pub(crate) mod curve;
pub(crate) mod keys;
mod knob;
mod label;
mod menu;
mod meter;
mod nodes;
pub(crate) mod notes;
mod number;
#[cfg(feature = "patcher")]
pub(crate) mod patch;
#[cfg(feature = "notation")]
mod score;
pub mod signal;
mod slider;
mod text;
mod toggle;

/// The constructor for a built-in element name, or `None` when no built-in
/// answers to it — in which case the caller consults the registry, and then
/// gives up on `Unknown`.
pub(crate) fn builtin(name: &str) -> Option<Constructor> {
    Some(match name {
        "button" => button::build,
        "canvas" => canvas::build,
        "curve" => curve::build,
        "keys" => keys::build,
        "knob" => knob::build,
        "label" => label::build,
        "meter" => meter::build,
        "menu" => menu::build,
        "nodes" => nodes::build,
        "notes" => notes::build,
        #[cfg(feature = "notation")]
        "score" => score::build,
        // Every view of a signal is one element: the props say which point of
        // the product it is (`elements::signal`), so one name answers for six.
        "signal" => signal::build,
        "number" => number::build,
        "slider" => slider::build,
        "text" => text::build,
        "toggle" => toggle::build,
        // Note what is **not** here: the patcher, whose wire type is `plane`
        // — the same name the scroll workspace answers to, told apart by the
        // props (`boxes`/`cords`). A name that means two constructions is the
        // schema's business, so `build_kind` resolves it and calls
        // [`patch::build`] directly, exactly as it does for a `field`.
        _ => return None,
    })
}
