//! `canvas` — a script's own fragment shader over the widget area.
//!
//! The leaf that proves an element can be a **heavy view**. It cannot draw into
//! the window's one mesh — a shader is not triangles the batch can carry — so
//! instead of implementing the mesh half of `draw` for its picture it *claims a
//! slot*, and the frame maintains that slot keyed by widget id exactly as it
//! does for a waveform's geometry or a spectrogram's texture.
//!
//! The set of slot kinds is the frame's and is closed, which is what keeps this
//! an ordinary element rather than an exemption: the user's WGSL is a
//! **parameter** of the shader slot, not a pipeline of its own, so a canvas
//! costs the pipeline the window already paid for and nothing per widget.
//!
//! Its uniforms are the second half. Four params ride the wire as numbers a
//! script sets, and any of them may instead name a control bus — resolved here,
//! per frame, from the world, which is the same read a meter does and costs the
//! same nothing. The picture follows the clock whatever the params do, so a
//! canvas declares itself animated and the window repaints for it.

use serde_json::{Map, Value};

use crate::host::canvas::{DEFAULT_SHADER, PARAM_COUNT};
use crate::host::controls;
use crate::host::font;
use crate::host::paint::Draw;
use crate::host::widget::element::{Ctx, Element, Needs, SlotFrame, SlotKind};
use crate::host::widget::parse;

/// A shader view. `params` are the four floats fed to the shader; a `buses`
/// slot that is not negative overrides its param from a control bus each frame.
#[derive(Debug, Clone)]
pub struct Canvas {
    pub shader: String,
    pub params: [f32; PARAM_COUNT],
    pub buses: [i32; PARAM_COUNT],
    pub label: Option<String>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `canvas` node carries, read once — shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Canvas {
    Canvas {
        shader: props
            .get("shader")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_SHADER.to_string()),
        params: parse::f32_array(props, "params", 0.0),
        buses: parse::i32_array(props, "buses", -1),
        label: parse::label(props),
    }
}

impl Canvas {
    /// The param vector as the shader sees it this frame: a `-1` slot keeps its
    /// script-set value, a bus slot is read from the world (zero messages, like
    /// a meter).
    fn resolved(&self, ctx: &Ctx) -> [f32; PARAM_COUNT] {
        let mut out = self.params;
        for (slot, &bus) in out.iter_mut().zip(self.buses.iter()) {
            if bus >= 0 {
                *slot = ctx.world.control(bus);
            }
        }
        out
    }
}

impl Element for Canvas {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "shader" => v.as_str().map(|s| self.shader = s.to_string()).is_some(),
            "label" => parse::set_label(&mut self.label, v),
            _ => {
                if let Some(i) = parse::index_suffix(key, "param").filter(|i| *i < PARAM_COUNT) {
                    parse::set_f(&mut self.params[i], v)
                } else if let Some(i) = parse::index_suffix(key, "bus").filter(|i| *i < PARAM_COUNT)
                {
                    v.as_i64().map(|n| self.buses[i] = n as i32).is_some()
                } else {
                    false
                }
            }
        }
    }

    /// Only the chrome: the picture is the slot's, and the label is the one
    /// part of a canvas that belongs in the shared mesh.
    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let Some(text) = self.label.as_deref() else {
            return;
        };
        let (mesh, m, theme) = d.parts();
        font::text(
            mesh,
            text,
            ctx.rect.x + m.pad,
            ctx.rect.y + m.pad,
            m.label_scale,
            theme.text,
        );
    }

    fn needs(&self) -> Needs {
        Needs {
            buses: self.buses.iter().copied().filter(|b| *b >= 0).collect(),
            // A shader's picture follows the clock whatever its params do, so a
            // canvas animates even with every slot script-set.
            animated: true,
            slot: Some(SlotKind::Shader {
                source: self.shader.clone(),
            }),
            ..Default::default()
        }
    }

    fn slot(&self, ctx: &Ctx) -> Option<SlotFrame> {
        Some(SlotFrame::Shader {
            body: controls::body_rect(ctx.rect, self.label.is_some(), ctx.metrics),
            source: self.shader.clone(),
            params: self.resolved(ctx),
        })
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::BusSource;
    use crate::host::layout::Rect;
    use crate::host::metrics::Metrics;
    use crate::host::world::World;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    /// A source that answers with the bus index, so a slot fed from the wrong
    /// bus is visible rather than plausible.
    struct Buses;

    impl BusSource for Buses {
        fn control(&self, index: usize) -> f32 {
            index as f32
        }
    }

    #[test]
    fn props_parse_and_default() {
        let c = from_props(&props(
            r#"{"shader":"fn shade(){}","params":[1,2],"buses":[-1,7],"label":"vis"}"#,
        ));
        assert_eq!(c.shader, "fn shade(){}");
        assert_eq!(c.params, [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(c.buses, [-1, 7, -1, -1]);
        assert_eq!(c.label.as_deref(), Some("vis"));

        let c = from_props(&props("{}"));
        assert_eq!(c.shader, DEFAULT_SHADER);
        assert_eq!(c.params, [0.0; PARAM_COUNT]);
        assert_eq!(c.buses, [-1; PARAM_COUNT]);
    }

    #[test]
    fn a_set_reaches_an_indexed_param_and_bus() {
        let mut c = from_props(&props("{}"));
        assert!(c.set("param2", &Value::from(0.5)));
        assert!(c.set("bus0", &Value::from(3)));
        assert!(c.set("shader", &Value::from("fn shade(){}")));
        assert_eq!(c.params[2], 0.5);
        assert_eq!(c.buses[0], 3);
        // Out of range and unknown alike are declined, not silently dropped.
        assert!(!c.set("param9", &Value::from(1.0)));
        assert!(!c.set("nonesuch", &Value::from(1)));
    }

    /// The claim is static — it is what a window allocates the slot from,
    /// before any frame — and the buses ride beside it as an ordinary
    /// declaration.
    #[test]
    fn it_claims_the_shader_slot_and_declares_its_buses() {
        let needs = from_props(&props(r#"{"shader":"fn shade(){}","buses":[4,-1,6]}"#)).needs();
        assert_eq!(
            needs.slot,
            Some(SlotKind::Shader {
                source: "fn shade(){}".into()
            })
        );
        assert_eq!(needs.buses, vec![4, 6], "an unset slot names no bus");
        assert!(needs.animated, "a shader's picture follows the clock");
    }

    /// The frame's half: a bus slot is resolved from the world this frame, a
    /// script-set one is kept, and the body is the rect minus the label strip.
    #[test]
    fn the_frame_resolves_its_params_from_the_world() {
        let buses = Buses;
        let world = World {
            bus: Some(&buses),
            ..Default::default()
        };
        let metrics = Metrics::default();
        let c = from_props(&props(r#"{"params":[9,9,9,9],"buses":[-1,2,-1,-1]}"#));
        let ctx = Ctx {
            world: &world,
            metrics: &metrics,
            rect: Rect::new(0.0, 0.0, 100.0, 60.0),
            indent: 0.0,
            scale: 1.0,
            time: None,
            clip: None,
            focused: false,
        };
        let Some(SlotFrame::Shader { body, params, .. }) = c.slot(&ctx) else {
            panic!("a canvas claims the shader slot");
        };
        assert_eq!(params, [9.0, 2.0, 9.0, 9.0]);
        assert_eq!(body, controls::body_rect(ctx.rect, false, &metrics));
    }
}
