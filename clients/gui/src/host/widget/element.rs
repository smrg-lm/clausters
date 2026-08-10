//! **The element seam**: one object-safe trait a leaf implements, and the
//! registry that maps a wire `type` onto it.
//!
//! The schema around this module is a closed sum type
//! ([`WidgetKind`](super::WidgetKind)) whose variants the whole renderer
//! matches on, which is right for the containers —
//! the layout pass has to know them — and wrong for the leaves: adding one
//! edits every pass, and a program *linking* this crate cannot add one at all.
//! This is the other door. A leaf implements [`Element`], registers a
//! constructor under a wire name ([`register`]), and the passes that are a
//! match arm for a built-in are a method call for it:
//!
//! | The pass a built-in spells as an arm | The element spells as |
//! |---|---|
//! | [`build`](super::build) | the registered [`Constructor`] |
//! | [`apply`](super::apply) | [`Element::set`] |
//! | [`size`](super::size) | [`Element::natural`] |
//! | the frame's flat draw | [`Element::draw`] |
//! | the query pass | [`Element::value`] / [`Element::info`] |
//! | the press walk | [`Element::press`] |
//! | the tree collectors | [`Element::needs`] |
//!
//! **The registry is consulted only when no built-in name matched**, so a
//! built-in never changes meaning and a third party can register an element
//! today, against a host where every leaf is still an enum arm. A registry
//! *miss* stays exactly what an unrecognized type has always been —
//! [`WidgetKind::Unknown`](super::WidgetKind::Unknown), laid out and not
//! painted — which is what makes an element family compilable out of a build
//! without a new failure mode: a slim host degrades the way an old host does.
//!
//! **Two boundaries, stated rather than discovered.** A **container is not
//! extensible here**: the layout pass owns the coordinate systems (`window`,
//! `layout`, `plane`, `field`, the clip), and a third-party coordinate system
//! is a different and much larger promise. And an element sees a **press**, not
//! the drag machine's internals: the ongoing drag is a state machine over
//! typed built-in drags ([`super::super::gestures`]), so an element claims the
//! press and mutates itself, which covers a click, a toggle and a discrete
//! pick.
//!
//! **The registry is per thread.** The host core is single-threaded by design
//! — nothing here is `Send`, which is exactly what makes `Box<dyn Element>`
//! cheap — so registrations live in a `thread_local!` and an element must be
//! registered on the thread that builds the trees (natively the one running
//! the event loop; in a page, the only one there is).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use super::super::layout::Rect;
use super::super::metrics::Metrics;
use super::super::paint::Draw;
use super::size::Natural;

/// What an element declares it reads from outside itself, collected by the
/// tree walks that feed a frame. Empty by default: an element that draws only
/// from its own props needs nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    /// Control buses read once per frame (the shm segment natively,
    /// `/bus_stream` in a page) — the same set a `meter` or a live scope
    /// contributes.
    pub buses: Vec<i32>,
}

/// What an element did with a press it was offered.
#[derive(Debug, Clone, PartialEq)]
pub enum Claim {
    /// Not this element's: hand the press back to the chain, exactly as a
    /// lane's empty space or a patcher's bare canvas does.
    Decline,
    /// Consumed. `value`, when present, is emitted on `/gui_event` as the
    /// widget's value — the same payload a built-in control delivers — and the
    /// window is redrawn either way.
    Take { value: Option<OscType> },
}

/// A leaf the renderer draws without knowing what it is.
///
/// Object-safe and single-threaded on purpose (see the module docs).
/// [`clone_box`](Element::clone_box) is the one piece of ceremony: the widget
/// tree is `Clone` (a def is rebuilt by replacement, and the frame copies out
/// of it), so a boxed element has to be too.
///
/// Every method but [`set`](Element::set), [`draw`](Element::draw) and
/// `clone_box` has a default, so the smallest element is three methods.
pub trait Element: fmt::Debug {
    /// Applies one `/gui_set` key/value, returning whether the key was this
    /// element's. A key it does not know must return `false` — the host logs
    /// the unknown prop rather than silently dropping it.
    fn set(&mut self, key: &str, v: &Value) -> bool;

    /// Draws into the window's one mesh, inside `rect`. `d` carries the
    /// resolved theme and the size table of the placement, so an element names
    /// roles and never literals, the way every built-in does.
    fn draw(&self, d: &mut Draw, rect: Rect);

    /// How big this element wants to be, per axis — `None` meaning elastic.
    /// Pure over the metrics, the element's own *presentation* props and the
    /// placement's `scale`, never over its data: a size that reads the data
    /// turns a `/gui_set` into a relayout. Elastic on both axes by default.
    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        (None, None)
    }

    /// This element's current value, for `/gui_event` and `/gui_query`.
    fn value(&self) -> Option<OscType> {
        None
    }

    /// Extra `/gui_query` fields, beside the value — an element's own state a
    /// script may want to read back.
    fn info(&self) -> Vec<(String, Value)> {
        Vec::new()
    }

    /// What this element reads from outside itself.
    fn needs(&self) -> Needs {
        Needs::default()
    }

    /// The press landed on this element at `at`, in the window's pixels, inside
    /// `rect`. Declining hands it back to the chain.
    fn press(&mut self, _at: (f64, f64), _rect: Rect) -> Claim {
        Claim::Decline
    }

    /// Clones this element into a fresh box (the tree is `Clone`).
    fn clone_box(&self) -> Box<dyn Element>;
}

impl Clone for Box<dyn Element> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Builds one element from the wire's props and the `/gui_def` message's
/// trailing blobs — the registered counterpart of a [`build`](super::build)
/// arm. An `Err` is a malformed node and is reported the way a built-in's is.
pub type Constructor = fn(&Map<String, Value>, &[Vec<u8>]) -> Result<Box<dyn Element>, String>;

thread_local! {
    static REGISTRY: RefCell<HashMap<String, Constructor>> = RefCell::new(HashMap::new());
}

/// Registers `ctor` under the wire `type` name, replacing any registration
/// under the same name (which is how a program overrides its own, and what
/// makes a test's registration repeatable).
///
/// A name that collides with a built-in is accepted and never consulted: the
/// built-ins are matched first, deliberately, so a registration cannot change
/// what an existing def means.
pub fn register(name: &str, ctor: Constructor) {
    REGISTRY.with(|r| r.borrow_mut().insert(name.to_string(), ctor));
}

/// Drops a registration, returning whether there was one.
pub fn unregister(name: &str) -> bool {
    REGISTRY.with(|r| r.borrow_mut().remove(name).is_some())
}

/// Builds the element registered under `name`, or `None` when nothing is —
/// a registry miss, which the caller turns into
/// [`WidgetKind::Unknown`](super::WidgetKind::Unknown).
pub(super) fn build_registered(
    name: &str,
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Option<Result<Box<dyn Element>, String>> {
    let ctor = REGISTRY.with(|r| r.borrow().get(name).copied())?;
    Some(ctor(props, blobs))
}

#[cfg(test)]
mod tests {
    //! The seam's own suite, driven the way a third party reaches it: register
    //! a constructor, then parse a `/gui_def` document that names it and put
    //! the result through the passes — because that round trip *is* what the
    //! trait promises.
    //!
    //! The registry is per thread and the harness is parallel, so every test
    //! registers what it needs; the names are test-local for the same reason.

    use super::super::super::guidef::GuiNode;
    use super::super::{Widget, WidgetKind};
    use super::*;

    /// The smallest complete element: a counter with a label, a value a script
    /// can set and a press that increments it.
    #[derive(Debug, Clone)]
    struct Counter {
        count: i32,
        bus: i32,
    }

    impl Element for Counter {
        fn set(&mut self, key: &str, v: &Value) -> bool {
            match key {
                "count" => v.as_i64().map(|n| self.count = n as i32).is_some(),
                "bus" => v.as_i64().map(|n| self.bus = n as i32).is_some(),
                _ => false,
            }
        }

        fn draw(&self, d: &mut Draw, rect: Rect) {
            let (mesh, _, theme) = d.parts();
            mesh.rect(rect, theme.panel);
        }

        fn natural(&self, m: &Metrics, scale: f32) -> Natural {
            (None, Some(m.control_h * scale))
        }

        fn value(&self) -> Option<OscType> {
            Some(OscType::Int(self.count))
        }

        fn info(&self) -> Vec<(String, Value)> {
            vec![("count".into(), Value::from(self.count))]
        }

        fn needs(&self) -> Needs {
            Needs {
                buses: vec![self.bus],
            }
        }

        fn press(&mut self, _at: (f64, f64), _rect: Rect) -> Claim {
            self.count += 1;
            Claim::Take {
                value: Some(OscType::Int(self.count)),
            }
        }

        fn clone_box(&self) -> Box<dyn Element> {
            Box::new(self.clone())
        }
    }

    fn counter(props: &Map<String, Value>, _blobs: &[Vec<u8>]) -> Result<Box<dyn Element>, String> {
        Ok(Box::new(Counter {
            count: props.get("count").and_then(Value::as_i64).unwrap_or(0) as i32,
            bus: props.get("bus").and_then(Value::as_i64).unwrap_or(-1) as i32,
        }))
    }

    fn tree(json: &str) -> Widget {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap()
    }

    /// The whole promise in one test: a name nothing built in answers to
    /// reaches the registry, and what comes back goes through the passes as a
    /// widget rather than as a hole in the tree.
    #[test]
    fn a_registered_name_builds_and_answers_every_pass() {
        register("test_counter", counter);
        let mut w = tree(r#"{"id":9,"type":"test_counter","count":3,"bus":7}"#);
        assert!(matches!(w.kind, WidgetKind::Custom(_)), "{:?}", w.kind);

        assert_eq!(w.kind.event_value(), Some(OscType::Int(3)));
        assert_eq!(w.kind.needs().buses, vec![7]);
        let m = Metrics::default();
        assert_eq!(w.kind.natural_size(&m, 1.0), (None, Some(m.control_h)));

        // A `/gui_set` lands on the element's own key; one it does not know is
        // reported as unhandled rather than swallowed.
        assert!(super::super::apply_widget(
            &mut w,
            "count",
            &Value::from(11)
        ));
        assert!(!super::super::apply_widget(
            &mut w,
            "nonesuch",
            &Value::from(1)
        ));
        assert_eq!(w.kind.event_value(), Some(OscType::Int(11)));

        unregister("test_counter");
    }

    /// A registry miss is what an unrecognized type has always been, which is
    /// the property that lets an element family be compiled out of a build: it
    /// degrades to the behavior of a host older than the def.
    #[test]
    fn a_miss_is_unknown_not_an_error() {
        let w = tree(r#"{"id":9,"type":"nothing_registered_here"}"#);
        assert!(
            matches!(w.kind, WidgetKind::Unknown(ref t) if t == "nothing_registered_here"),
            "{:?}",
            w.kind
        );
    }

    /// The built-ins are matched first, so a registration under a name one
    /// already answers to is inert — it cannot change what a shipped def means.
    #[test]
    fn a_registration_never_shadows_a_built_in() {
        register("label", counter);
        let w = tree(r#"{"id":9,"type":"label","text":"hello"}"#);
        assert!(matches!(w.kind, WidgetKind::Label { .. }), "{:?}", w.kind);
        unregister("label");
    }

    /// The tree is `Clone` (a def is rebuilt by replacement and a frame copies
    /// out of it), so a boxed element clones deeply rather than aliasing.
    #[test]
    fn a_boxed_element_clones_deeply() {
        register("test_clone", counter);
        let mut w = tree(r#"{"id":9,"type":"test_clone","count":1}"#);
        let copy = w.clone();
        super::super::apply_widget(&mut w, "count", &Value::from(5));
        assert_eq!(w.kind.event_value(), Some(OscType::Int(5)));
        assert_eq!(copy.kind.event_value(), Some(OscType::Int(1)));
        unregister("test_clone");
    }

    /// A constructor that rejects its props fails the def the way a malformed
    /// built-in node does, rather than being reported as an unknown type.
    #[test]
    fn a_constructor_error_fails_the_def() {
        fn refuses(
            _props: &Map<String, Value>,
            _blobs: &[Vec<u8>],
        ) -> Result<Box<dyn Element>, String> {
            Err("no".into())
        }
        register("test_refuses", refuses);
        let node = GuiNode::parse(br#"{"id":9,"type":"test_refuses"}"#).unwrap();
        assert_eq!(Widget::from_node(1, &node, &[]).err(), Some("no".into()));
        unregister("test_refuses");
    }
}
