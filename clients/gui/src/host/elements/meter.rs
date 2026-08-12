//! `meter` — a bus level as a column.
//!
//! The smallest thing that reads the **world**: one bus, one read per frame,
//! nothing else. Which table that read lands in is the `rate` prop — an audio
//! bus publishes one block level, a control bus carries a value — and both are
//! one atomic load out of the same source, so a meter costs neither a message
//! nor a recording. That is also the whole of what it declares: a control-rate
//! meter contributes its bus to the frame's stream subscription, an audio-rate
//! one contributes a level, and the window animates because of that
//! declaration rather than because a collector knew the word `meter`.

use serde_json::{Map, Value};

use crate::host::graphics::meters;
use crate::host::paint::Draw;
use crate::host::widget::element::{Ctx, Element, Needs};
use crate::host::widget::{Rate, parse};

/// A level meter over one bus, drawn as a column filling `min`..`max`.
#[derive(Debug, Clone)]
pub struct Meter {
    pub bus: i32,
    /// Audio rate reads the bus's published block level; control rate reads the
    /// control bus's current value.
    pub rate: Rate,
    pub min: f32,
    pub max: f32,
    pub label: Option<String>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `meter` node carries, read once — shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Meter {
    Meter {
        bus: parse::int_prop(props, "bus", 0),
        rate: Rate::parse(props.get("rate").and_then(Value::as_str)),
        min: parse::number(props, "min", 0.0),
        max: parse::number(props, "max", 1.0),
        label: parse::label(props),
    }
}

impl Element for Meter {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "bus" => v.as_i64().map(|n| self.bus = n as i32).is_some(),
            "rate" => parse::set_rate(&mut self.rate, v),
            "min" => parse::set_f(&mut self.min, v),
            "max" => parse::set_f(&mut self.max, v),
            "label" => parse::set_label(&mut self.label, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let value = ctx.world.level(self.bus, self.rate);
        let fraction = meters::fraction(value, self.min, self.max);
        meters::draw_meter(d, ctx.rect, value, fraction, self.label.as_deref());
    }

    fn needs(&self) -> Needs {
        // The rate picks the table, so it picks the declaration: samples are
        // never recorded for a meter, at either rate.
        let bus = vec![self.bus];
        match self.rate {
            Rate::Audio => Needs {
                levels: bus,
                ..Default::default()
            },
            Rate::Control => Needs {
                buses: bus,
                ..Default::default()
            },
        }
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::BusSource;
    use crate::host::world::World;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// A source whose two tables answer differently, so a read that went to the
    /// wrong one is visible rather than plausible.
    struct Buses;

    impl BusSource for Buses {
        fn control(&self, index: usize) -> f32 {
            index as f32 / 10.0
        }

        fn level(&self, bus: i32) -> f32 {
            bus as f32 / 100.0
        }
    }

    #[test]
    fn props_parse_and_default() {
        let m = from_props(&props(
            r#"{"bus":4,"rate":"audio","min":-1,"max":2,"label":"out"}"#,
        ));
        assert_eq!((m.bus, m.rate), (4, Rate::Audio));
        assert_eq!((m.min, m.max), (-1.0, 2.0));
        assert_eq!(m.label.as_deref(), Some("out"));

        let m = from_props(&props("{}"));
        assert_eq!(
            (m.bus, m.rate),
            (0, Rate::Audio),
            "a meter watches audio unless told"
        );
        assert_eq!((m.min, m.max), (0.0, 1.0));
        assert_eq!(m.label, None);
    }

    #[test]
    fn a_set_lands_on_its_own_key_and_declines_the_rest() {
        let mut m = from_props(&props("{}"));
        assert!(m.set("bus", &Value::from(7)));
        assert!(m.set("rate", &Value::from("control")));
        assert!(m.set("max", &Value::from(4.0)));
        assert!(m.set("label", &Value::from("in")));
        assert_eq!((m.bus, m.rate, m.max), (7, Rate::Control, 4.0));
        assert!(!m.set("nonesuch", &Value::from(1)));
    }

    /// The declaration *is* the rate: a control-rate meter asks for its bus to
    /// be streamed, an audio-rate one asks for a published level, and neither
    /// ever asks the server to record samples.
    #[test]
    fn the_rate_picks_what_is_declared() {
        let control = from_props(&props(r#"{"bus":3,"rate":"control"}"#)).needs();
        assert_eq!(control.buses, vec![3]);
        assert!(control.levels.is_empty() && control.taps.is_empty());

        let audio = from_props(&props(r#"{"bus":3}"#)).needs();
        assert_eq!(audio.levels, vec![3]);
        assert!(audio.buses.is_empty() && audio.taps.is_empty());
    }

    /// And the rate picks the table the *draw* reads, which is the same
    /// question asked of the world instead of of a collector.
    #[test]
    fn the_rate_picks_the_table_read() {
        let buses = Buses;
        let world = World {
            bus: Some(&buses),
            ..Default::default()
        };
        let read = |json: &str| {
            let m = from_props(&props(json));
            world.level(m.bus, m.rate)
        };
        assert_eq!(read(r#"{"bus":5,"rate":"control"}"#), 0.5);
        assert_eq!(read(r#"{"bus":5}"#), 0.05);
    }
}
