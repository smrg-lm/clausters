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
//! It is also the seam the crate's features will hang on: a family compiled out
//! drops its rows from this table, the lookup falls through to the registry and
//! then to `Unknown` — laid out, not painted — which is exactly how a host
//! older than the def already behaves. No new failure mode, and nothing for a
//! script to learn.

use super::widget::element::Constructor;

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
mod number;
mod score;
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
        "score" => score::build,
        // Every view of a signal is one element: the props say which point of
        // the product it is (`host::signal`), so one name answers for six.
        "signal" => super::signal::build,
        "number" => number::build,
        "slider" => slider::build,
        "text" => text::build,
        "toggle" => toggle::build,
        _ => return None,
    })
}
