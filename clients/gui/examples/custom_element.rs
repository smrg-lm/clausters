//! An element of one's own, in a window of one's own: the host linked as a
//! library, with no script on the other end.
//!
//! A program that links `clausters-gui` has two doors, and this example goes
//! through both and through nothing else -- no Python, no OSC, no JSON:
//!
//! - [`clausters_gui::register`] adds a leaf the catalog does not have. The
//!   leaf is a type implementing [`Element`], registered under the `type` name
//!   a def will call it by;
//! - [`clausters_gui::tree`] builds the window from Rust values, and
//!   [`Host::define`] takes it.
//!
//! The element here is a **step pad**: a bar of `steps` cells, lit up to
//! `count`. A press lights the next one and wraps after the last. It answers
//! the four questions every element is asked -- what a prop means (`set`), how
//! big it wants to be (`natural`), what it looks like (`draw`) and what a
//! press does (`press`) -- and reports its count as its value, so whoever
//! listens to the window hears `/gui_event <id> <count>` at each press.
//!
//! What it leaves out is everything it does not need, which is the point of
//! the trait's defaults: no axis, no samples, no keys, no drag.
//!
//! Run it from `clients/gui`, on a machine with a display:
//!
//! ```sh
//! cargo run --example custom_element
//! ```
//!
//! Press a pad to step it. Close the window to end the run.

use std::net::UdpSocket;
use std::sync::Arc;

use clausters_core::osc::OscType;
use clausters_gui::element::{Claim, Ctx, Input};
use clausters_gui::host::Host;
use clausters_gui::host::layout::Rect;
use clausters_gui::host::metrics::Metrics;
use clausters_gui::host::paint::Draw;
use clausters_gui::{Element, register, tree};
use serde_json::{Map, Value};

/// The wire name the pad is registered under, and what a def calls it by.
const STEP_PAD: &str = "step_pad";

/// A bar of `steps` cells, lit up to `count`.
#[derive(Debug, Clone)]
struct StepPad {
    steps: i32,
    count: i32,
}

impl StepPad {
    /// Built from a def's props: the registered counterpart of a built-in's
    /// parse. A prop the pad does not know is ignored, as the host ignores it
    /// on every widget.
    fn from_props(
        props: &Map<String, Value>,
        _blobs: &[Vec<u8>],
    ) -> Result<Box<dyn Element>, String> {
        let mut pad = StepPad { steps: 4, count: 0 };
        for (key, value) in props {
            pad.set(key, value);
        }
        Ok(Box::new(pad))
    }
}

impl Element for StepPad {
    /// One prop, applied: at construction and at every `/gui_set` alike, so a
    /// script that sets `count` on an open window reaches the same code.
    fn set(&mut self, key: &str, v: &Value) -> bool {
        let Some(n) = v.as_i64() else {
            return false;
        };
        match key {
            "steps" => self.steps = (n as i32).max(1),
            "count" => self.count = n as i32,
            _ => return false,
        }
        self.count = self.count.rem_euclid(self.steps + 1);
        true
    }

    /// One control high and as wide as it is given: the size roles come from
    /// the host's table, so the pad follows the window's density.
    fn natural(&self, m: &Metrics, scale: f32) -> (Option<f32>, Option<f32>) {
        (None, Some(m.control_h * scale))
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let (mesh, m, theme) = d.parts();
        let gap = m.gap;
        let cell = (ctx.rect.w - gap * (self.steps - 1) as f32) / self.steps as f32;
        for step in 0..self.steps {
            let r = Rect {
                x: ctx.rect.x + step as f32 * (cell + gap),
                y: ctx.rect.y,
                w: cell,
                h: ctx.rect.h,
            };
            let lit = step < self.count;
            mesh.rect(r, if lit { theme.accent } else { theme.panel });
        }
    }

    /// The value the window reports for this widget, and what `/gui_query`
    /// reads back.
    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(self.count))
    }

    /// A press anywhere on the pad lights the next cell. Taking the press is
    /// what redraws the window, and the value is what goes out as the event.
    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        self.count = (self.count + 1).rem_euclid(self.steps + 1);
        Claim::value(OscType::Int(self.count))
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

fn main() -> Result<(), String> {
    // The registration is per thread, and the host builds its trees on the
    // thread that runs it: register here, before the first def names the pad.
    register(STEP_PAD, StepPad::from_props);

    let mut host = Host::new();
    host.define(
        1,
        tree::window()
            .prop("title", "A registered element")
            .prop("w", 420)
            .prop("h", 160)
            .prop("layout", "col")
            .child(tree::node("label").id(2).prop("text", "press a pad"))
            .child(tree::node(STEP_PAD).id(3).prop("steps", 4))
            .child(tree::node(STEP_PAD).id(4).prop("steps", 8).prop("count", 3)),
    );

    // The windowed front wants a socket for the scripts it could be driven by;
    // nothing sends to this one. No shared-memory segment, no TCP, no
    // WebSocket: the window is the whole program.
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    clausters_gui::host::gui::run(host, Arc::new(socket), None, None, None)
}
