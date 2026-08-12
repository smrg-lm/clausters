//! `keys` — a MIDI keyboard: a range of keys, the ones held down, and an
//! overview strip that navigates the range.
//!
//! **The leaf that navigates an axis of its own and does not join a group.** A
//! piano measures pitch along x, so there is nothing to share with the window's
//! time — its range is its own two numbers, moved by the wheel and by a drag on
//! the overview strip, and reported as a `"range"` event the way a timeline
//! view reports a `"view"`.
//!
//! Its two drags are both **snapshotted**: the press records what it needs (the
//! strip and the range at that moment, or the key layout and the pitch under
//! the cursor) and every motion is measured against that snapshot rather than
//! accumulated, so a drag pinned at an end never drifts. They live in the
//! element because the state is the element's — which is the whole reason the
//! machine keeps no taxonomy of drags.
//!
//! **Voice mode is the one thing it cannot do for itself.** With `voice` set,
//! a held key sounds a server def, and only the host has a leg to the server.
//! So the element *asks*: a note it reports carries a [`Voice`] request beside
//! it, and the host starts or gates the synth. That is the same shape as the
//! pointer grab a knob asks for — what only the front can do, named in what the
//! element returns.

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::graphics::piano;
use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Voice, VoiceSpec};
use crate::host::widget::parse;

/// A keyboard. `pressed` and `drag` are native view state — the gestures build
/// them, the drawing reads them, and no `/gui_set` writes them.
#[derive(Debug, Clone)]
pub struct Keys {
    pub min: i32,
    pub max: i32,
    pub active_min: i32,
    pub active_max: i32,
    pub pan: bool,
    pub overview: bool,
    /// A fixed press velocity; `None` maps velocity from the press height.
    pub velocity: Option<i32>,
    /// The MIDI channel carried in the `"note"` event (0..15).
    pub channel: i32,
    /// Host-voice mode: the server def one voice per held key plays.
    pub voice: Option<String>,
    /// Extra `/synth_new` control pairs appended after `freq`/`amp`/`gate`.
    pub voice_args: Vec<(String, f32)>,
    /// The held keys.
    pub pressed: Vec<i32>,
    pub label: Option<String>,
    /// The drag in flight, with the snapshot it is measured against.
    drag: Option<Drag>,
}

/// What a held press on a keyboard is doing.
#[derive(Debug, Clone)]
enum Drag {
    /// A key is sounding, and crossing into another one hands the note over
    /// (the glissando). The layout is the one the press was measured on, so
    /// the keys do not move under the finger.
    Key { layout: piano::Layout, pitch: i32 },
    /// The overview strip is panning the visible range, from the range and the
    /// key under the press.
    View {
        strip: Rect,
        min0: i32,
        max0: i32,
        anchor: i32,
    },
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `keys` node carries, read once — shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Keys {
    let min = parse::number(props, "min", 36.0) as i32;
    let max = parse::number(props, "max", 96.0) as i32;
    Keys {
        min: piano::snap_white_down(min.min(max).clamp(0, 127)),
        max: max.max(min).clamp(0, 127),
        active_min: parse::number(props, "active_min", 0.0) as i32,
        active_max: parse::number(props, "active_max", 127.0) as i32,
        pan: props.get("pan").and_then(parse::truthy).unwrap_or(true),
        overview: props
            .get("overview")
            .and_then(parse::truthy)
            .unwrap_or(true),
        // Absent or negative = dynamic (mapped from the press height).
        velocity: props
            .get("velocity")
            .and_then(Value::as_i64)
            .filter(|&v| v >= 0)
            .map(|v| (v as i32).clamp(1, 127)),
        channel: (parse::number(props, "channel", 0.0) as i32).clamp(0, 15),
        voice: props
            .get("voice")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        voice_args: parse::voice_args(props),
        pressed: Vec::new(),
        label: parse::label(props),
        drag: None,
    }
}

impl Keys {
    /// The key geometry for a placement — the same one the renderer drew with,
    /// so a press lands on the key that was painted.
    fn layout(&self, rect: Rect, m: &crate::host::metrics::Metrics) -> piano::Layout {
        piano::layout(
            rect,
            self.min,
            self.max,
            self.overview,
            self.label.is_some(),
            m,
        )
    }

    /// Sounds `pitch` at `velocity`: the key joins the held set, and the note
    /// leaves as the MIDI-shaped payload plus the voice request the host
    /// performs.
    fn note_on(&mut self, pitch: i32, velocity: i32) -> Events {
        if !self.pressed.contains(&pitch) {
            self.pressed.push(pitch);
        }
        Events::message(note_args(pitch, velocity, 1, self.channel))
            .and_voice(Voice::on(pitch, velocity))
    }

    /// Releases `pitch`, whether or not it was held — a range change can drop a
    /// key mid-hold, and the release must still reach the script and the voice.
    fn note_off(&mut self, pitch: i32) -> Events {
        self.pressed.retain(|&p| p != pitch);
        Events::message(note_args(pitch, 0, 0, self.channel)).and_voice(Voice::off(pitch))
    }

    /// Moves the visible range: the low end snaps to a white key, held keys
    /// that left the window drop (their rects are gone), and the `"range"`
    /// event is reported **only when it moved** — a wheel at the end of the
    /// axis is not an edit.
    fn set_range(&mut self, min: i32, max: i32) -> Events {
        let lo = piano::snap_white_down(min.clamp(0, 127).min(max));
        let hi = max.clamp(0, 127).max(lo);
        if (lo, hi) == (self.min, self.max) {
            return Events::none();
        }
        (self.min, self.max) = (lo, hi);
        self.pressed.retain(|p| (lo..=hi).contains(p));
        // Always an event, never a bound forward: a binding carries the note
        // payload, where the range is view state.
        Events::message(vec![
            OscType::String("range".into()),
            OscType::Int(lo),
            OscType::Int(hi),
        ])
    }
}

/// The MIDI-shaped `"note" pitch velocity state channel` payload (state 1 = on,
/// 0 = off) — what a `/gui_event` carries and what a bound keyboard forwards.
fn note_args(pitch: i32, velocity: i32, state: i32, channel: i32) -> Vec<OscType> {
    vec![
        OscType::String("note".into()),
        OscType::Int(pitch),
        OscType::Int(velocity),
        OscType::Int(state),
        OscType::Int(channel),
    ]
}

impl Element for Keys {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // A range change re-normalizes (min white-snapped) and drops held
            // keys that left the visible window (their rects are gone; the
            // release gesture tolerates the miss).
            "min" => v
                .as_i64()
                .map(|n| {
                    self.min = piano::snap_white_down((n as i32).clamp(0, 127).min(self.max));
                    self.pressed.retain(|p| *p >= self.min);
                })
                .is_some(),
            "max" => v
                .as_i64()
                .map(|n| {
                    self.max = (n as i32).clamp(0, 127).max(self.min);
                    self.pressed.retain(|p| *p <= self.max);
                })
                .is_some(),
            "active_min" => v.as_i64().map(|n| self.active_min = n as i32).is_some(),
            "active_max" => v.as_i64().map(|n| self.active_max = n as i32).is_some(),
            "pan" => parse::truthy(v).map(|b| self.pan = b).is_some(),
            "overview" => parse::truthy(v).map(|b| self.overview = b).is_some(),
            // A negative velocity restores the dynamic (press-height) map.
            "velocity" => v
                .as_i64()
                .map(|n| self.velocity = (n >= 0).then(|| (n as i32).clamp(1, 127)))
                .is_some(),
            "channel" => v
                .as_i64()
                .map(|n| self.channel = (n as i32).clamp(0, 15))
                .is_some(),
            // An empty name leaves voice mode; the sounding keys are gated by
            // the release either way (`piano_voice_off` tolerates the miss).
            "voice" => v
                .as_str()
                .map(|s| self.voice = (!s.is_empty()).then(|| s.to_string()))
                .is_some(),
            // The flat `[name, value, …]` array rides as its JSON string, the
            // scalar carrier a `/gui_set` of a non-scalar always uses.
            "voice_args" => {
                self.voice_args = parse::voice_args(&parse::as_array_props("voice_args", v));
                true
            }
            "label" => parse::set_label(&mut self.label, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        piano::draw_widget(
            d,
            ctx.rect,
            self.min,
            self.max,
            self.overview,
            self.active_min,
            self.active_max,
            &self.pressed,
            self.label.as_deref(),
        );
    }

    /// The strip grabs the range, a key sounds. A press outside the **active**
    /// range is inert but still taken: the keyboard is what is under the
    /// cursor, and letting the press fall through would pan whatever is behind
    /// it.
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let l = self.layout(input.rect, input.metrics);
        if let Some(strip) = l.overview.filter(|s| s.contains(at.0, at.1)) {
            if self.pan {
                self.drag = Some(Drag::View {
                    strip,
                    min0: l.min,
                    max0: l.max,
                    anchor: piano::overview_hit(strip, at.0 as f32),
                });
            }
            return Claim::take();
        }
        let Some(pitch) = piano::hit(&l, at.0 as f32, at.1 as f32) else {
            return Claim::take();
        };
        if !(self.active_min..=self.active_max).contains(&pitch) {
            return Claim::take();
        }
        let velocity = self
            .velocity
            .unwrap_or_else(|| piano::velocity_at(&l, pitch, at.1 as f32));
        let events = self.note_on(pitch, velocity);
        self.drag = Some(Drag::Key { layout: l, pitch });
        Claim::events(events)
    }

    fn drag(&mut self, at: (f64, f64), _input: &Input) -> Events {
        match self.drag.clone() {
            // Glissando: crossing into another (active) key releases the held
            // one and presses the new; leaving the keyboard keeps the note held
            // until release.
            Some(Drag::Key { layout, pitch }) => {
                let Some(p) = piano::hit(&layout, at.0 as f32, at.1 as f32) else {
                    return Events::none();
                };
                if p == pitch || !(self.active_min..=self.active_max).contains(&p) {
                    return Events::none();
                }
                let velocity = self
                    .velocity
                    .unwrap_or_else(|| piano::velocity_at(&layout, p, at.1 as f32));
                let off = self.note_off(pitch);
                let on = self.note_on(p, velocity);
                self.drag = Some(Drag::Key { layout, pitch: p });
                off.chain(on)
            }
            Some(Drag::View {
                strip,
                min0,
                max0,
                anchor,
            }) => {
                let cur = piano::overview_hit(strip, at.0 as f32);
                let (min, max) = piano::pan_range(min0, max0, cur - anchor);
                self.set_range(min, max)
            }
            None => Events::none(),
        }
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        match self.drag.take() {
            Some(Drag::Key { pitch, .. }) => self.note_off(pitch),
            _ => Events::none(),
        }
    }

    /// The keyboard navigates **its own MIDI range**: over the overview strip
    /// the wheel zooms it anchored at the key under the cursor, over the keys
    /// it pans by whole white keys. Both gated by `pan`; a fixed-range keyboard
    /// lets the wheel through to whatever is behind it.
    fn wheel(&mut self, at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        if !self.pan {
            return None;
        }
        let steps = delta.1;
        let l = self.layout(input.rect, input.metrics);
        let (min, max) = match l.overview.filter(|s| s.contains(at.0, at.1)) {
            Some(strip) => {
                let anchor = piano::overview_hit(strip, at.0 as f32) as f64;
                piano::zoom_range(l.min, l.max, 0.85f64.powf(steps), anchor)
            }
            None => piano::pan_white(l.min, l.max, steps.round() as i32),
        };
        Some(self.set_range(min, max))
    }

    /// The def one held key sounds, when this keyboard is in voice mode.
    fn voice(&self) -> Option<VoiceSpec> {
        Some(VoiceSpec {
            def: self.voice.clone()?,
            args: self.voice_args.clone(),
        })
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn keys(json: &str) -> Keys {
        from_props(&props(json))
    }

    /// A press is measured on the rect the element was drawn in, at the size
    /// table it was drawn with — the same `Input` every element gets.
    fn input(rect: Rect, m: &Metrics) -> Input<'_> {
        Input {
            metrics: m,
            indent: 0.0,
            rect,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (rect.w, rect.h),
            time: None,
        }
    }

    #[test]
    fn parses_defaults_and_normalizes_the_range() {
        let k = keys("{}");
        assert_eq!((k.min, k.max), (36, 96));
        assert_eq!((k.active_min, k.active_max), (0, 127));
        assert!(k.pan && k.overview);
        assert_eq!(k.velocity, None); // dynamic (press-height) velocity
        assert_eq!(k.channel, 0);
        assert!(k.voice.is_none() && k.voice_args.is_empty());
        assert!(k.pressed.is_empty());

        // A black-key min snaps down to its white key; voice props parse.
        let k = keys(
            r#"{"min":61,"max":85,"velocity":90,"channel":3,
                "voice":"pv","voice_args":["pan",0.5]}"#,
        );
        assert_eq!(k.min, 60);
        assert_eq!(k.velocity, Some(90));
        assert_eq!(k.channel, 3);
        assert_eq!(k.voice.as_deref(), Some("pv"));
        assert_eq!(k.voice_args, [("pan".to_string(), 0.5f32)]);
    }

    #[test]
    fn apply_round_trips_and_prunes_held_keys() {
        let mut k = keys(r#"{"min":48,"max":84}"#);
        k.pressed.extend([50, 80]);
        // A narrowed range white-snaps its min and drops held keys outside it.
        assert!(k.set("min", &Value::from(61)));
        assert!(k.set("max", &Value::from(72)));
        assert_eq!((k.min, k.max), (60, 72));
        assert!(k.pressed.is_empty());

        // A negative velocity restores the dynamic map; an empty voice unsets.
        assert!(k.set("velocity", &Value::from(100)));
        assert!(k.set("velocity", &Value::from(-1)));
        assert!(k.set("voice", &Value::from("pv")));
        assert!(k.set("voice", &Value::from("")));
        assert!(k.set("pan", &Value::from(0)));
        assert!(k.set("active_min", &Value::from(40)));
        // `voice_args` rides as the JSON-string scalar carrier, like `notes`.
        assert!(k.set("voice_args", &Value::from("[\"pan\",0.25]")));
        assert_eq!(k.velocity, None);
        assert!(k.voice.is_none());
        assert_eq!(k.voice_args, [("pan".to_string(), 0.25f32)]);
        assert!(!k.pan);
        assert_eq!(k.active_min, 40);
        assert!(!k.set("nonesuch", &Value::from(1)));
    }

    /// A key sounds on the press and stops on the release, and the note leaves
    /// **twice**: as the MIDI payload a script reads, and as the voice request
    /// the host performs. The two travel together so a bound keyboard and a
    /// sounding one cannot disagree about what was played.
    #[test]
    fn a_press_sounds_a_key_and_the_release_stops_it() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 400.0, 120.0);
        let mut k = keys(r#"{"min":60,"max":72,"velocity":100,"channel":2}"#);
        let l = k.layout(rect, &m);
        let c = piano::key_rect(&l, 62).expect("D is on screen");
        let at = ((c.x + c.w * 0.5) as f64, (c.y + c.h * 0.8) as f64);

        let Claim::Take(take) = k.press(at, &input(rect, &m)) else {
            panic!("a press on a key is taken")
        };
        assert_eq!(k.pressed, [62]);
        assert_eq!(
            take.events.clone().into_messages(),
            vec![note_args(62, 100, 1, 2)]
        );
        assert_eq!(take.events.voices(), [Voice::on(62, 100)]);

        let events = k.release(at, &input(rect, &m));
        assert!(k.pressed.is_empty());
        assert_eq!(events.clone().into_messages(), vec![note_args(62, 0, 0, 2)]);
        assert_eq!(events.voices(), [Voice::off(62)]);
    }

    /// The glissando: crossing into another key hands the note over — one off,
    /// one on, in that order, so nothing is left sounding.
    #[test]
    fn a_drag_across_keys_hands_the_note_over() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 400.0, 120.0);
        let mut k = keys(r#"{"min":60,"max":72,"velocity":80}"#);
        let l = k.layout(rect, &m);
        let (c, d) = (
            piano::key_rect(&l, 60).unwrap(),
            piano::key_rect(&l, 62).unwrap(),
        );
        let on_c = ((c.x + c.w * 0.5) as f64, (c.y + c.h * 0.8) as f64);
        let on_d = ((d.x + d.w * 0.5) as f64, (d.y + d.h * 0.8) as f64);
        k.press(on_c, &input(rect, &m));
        let events = k.drag(on_d, &input(rect, &m));
        assert_eq!(k.pressed, [62], "only the new key is held");
        assert_eq!(
            events.clone().into_messages(),
            vec![note_args(60, 0, 0, 0), note_args(62, 80, 1, 0)]
        );
        assert_eq!(events.voices(), [Voice::off(60), Voice::on(62, 80)]);
        // And staying on the same key says nothing at all.
        assert!(k.drag(on_d, &input(rect, &m)).is_empty());
    }

    /// A press outside the **active** range is inert but still taken: the
    /// keyboard is what the reader pointed at, and letting the press through
    /// would pan whatever is behind it.
    #[test]
    fn a_key_outside_the_active_range_sounds_nothing() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 400.0, 120.0);
        let mut k = keys(r#"{"min":60,"max":72,"active_min":64,"active_max":72}"#);
        let l = k.layout(rect, &m);
        let c = piano::key_rect(&l, 60).unwrap();
        let claim = k.press(
            ((c.x + c.w * 0.5) as f64, (c.y + c.h * 0.8) as f64),
            &input(rect, &m),
        );
        assert_eq!(claim, Claim::take());
        assert!(k.pressed.is_empty());
    }

    /// The wheel navigates the range and reports it — and a wheel at the end
    /// of the axis reports **nothing**, because a gesture that moves nothing
    /// says nothing.
    #[test]
    fn the_wheel_pans_the_range_and_a_stuck_axis_says_nothing() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 400.0, 120.0);
        let mut k = keys(r#"{"min":60,"max":72}"#);
        let over_keys = (200.0, 100.0);
        let events = k
            .wheel(over_keys, (0.0, 1.0), &input(rect, &m))
            .expect("a pannable keyboard takes the wheel");
        assert!(!events.is_empty(), "the range moved, so it is reported");
        assert_ne!((k.min, k.max), (60, 72));

        // Pinned at the bottom of the MIDI range: no movement, no event.
        let mut k = keys(r#"{"min":0,"max":12}"#);
        let events = k.wheel(over_keys, (0.0, -1.0), &input(rect, &m)).unwrap();
        assert!(events.is_empty());
        assert_eq!((k.min, k.max), (0, 12));

        // A fixed-range keyboard declines, so the wheel reaches whatever is
        // behind it.
        let mut fixed = keys(r#"{"min":60,"max":72,"pan":0}"#);
        assert!(
            fixed
                .wheel(over_keys, (0.0, 1.0), &input(rect, &m))
                .is_none()
        );
    }

    /// Voice mode is a **declaration**: the element says which def a held key
    /// plays, and the host reads it when it performs the request.
    #[test]
    fn voice_mode_is_declared_not_performed() {
        assert!(keys("{}").voice().is_none());
        let k = keys(r#"{"voice":"pv","voice_args":["pan",0.5]}"#);
        let spec = k.voice().expect("a voice was named");
        assert_eq!(spec.def, "pv");
        assert_eq!(spec.args, [("pan".to_string(), 0.5f32)]);
    }
}
