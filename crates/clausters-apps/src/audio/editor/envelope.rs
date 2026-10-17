//! The take's gain curve, projected through the existing points vocabulary.

use super::*;
use clausters_document::NodeId;
use clausters_document::multitrack::{Automation, edit::MultitrackIntent};
use clausters_document::timebase::Second;
use clausters_editing::points;

impl AudioEditor {
    pub(super) fn preview_state(&self) -> Value {
        json!({"on":self.preview_on, "installed":self.effect_installed,
            "points":self.envelope_quads(false),"revision":self.contents.payload().0})
    }

    pub(super) fn preview_event(
        &mut self,
        verb: &str,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        self.ensure_envelope();
        match verb {
            "preview_on" => {
                self.preview_on = true;
                self.effect_installed = true;
            }
            "preview_off" => self.preview_on = false,
            "remove_effect" => {
                self.preview_on = false;
                self.effect_installed = false;
            }
            _ => unreachable!(),
        }
        out.preview = Some(self.preview_state());
        (None, Vec::new())
    }

    pub(super) fn pane_label(&self) -> String {
        let take = crate::samples::label(self.name.as_deref(), self.display);
        if self.edit_layer.starts_with("points") {
            format!("{take} -- gain 0..2 (unity 1)")
        } else {
            take
        }
    }

    pub(super) fn ensure_envelope(&mut self) {
        if self.contents.automation.is_empty() {
            let mut curve =
                Automation::new(NodeId(1), Opaque(json!({"port":"gain", "min":0, "max":2})));
            curve.points = vec![
                points::point(Second(0.0), 1.0, 1.0, 0.0),
                points::point(Second(self.length() as f64 / self.rate), 1.0, 1.0, 0.0),
            ];
            self.contents.automation.push(curve);
        }
    }

    pub(super) fn envelope_quads(&self, frames: bool) -> Vec<f64> {
        self.contents.automation.first().map_or_else(Vec::new, |a| {
            a.points
                .iter()
                .flat_map(|p| points::quad(p.at.0 * if frames { self.rate } else { 1.0 }, p))
                .collect()
        })
    }

    fn envelope_props(&self) -> Value {
        let mut props = points::props(
            &self.envelope_quads(true),
            Some((0.0, 2.0)),
            self.length() as f64,
        );
        props.insert("min".into(), json!(0));
        props.insert("max".into(), json!(2));
        props.insert("editable".into(), json!(true));
        Value::Object(props)
    }

    pub(super) fn envelope_corrections(&self) -> Vec<Correction> {
        self.envelope_widget
            .into_iter()
            .map(|widget| Correction {
                widget: i64::from(widget),
                props: self.envelope_props(),
            })
            .collect()
    }

    pub(super) fn compose_envelope(&self, tree: &mut Value) {
        if let (Some(id), Some(pane)) = (self.envelope_widget, self.pane) {
            fn append(tree: &mut Value, pane: i32, body: &Value, visible: bool) {
                if tree.get("id").and_then(Value::as_i64) == Some(i64::from(pane)) {
                    tree["children"].as_array_mut().unwrap().push(body.clone());
                    tree["hidden"] = json!(if visible { "" } else { "points" });
                } else if let Some(children) =
                    tree.get_mut("children").and_then(Value::as_array_mut)
                {
                    for child in children {
                        append(child, pane, body, visible);
                    }
                }
            }
            let mut body = self.envelope_props();
            body["type"] = json!("curve");
            body["id"] = json!(id);
            append(
                tree,
                pane,
                &body,
                self.contents.automation.first().is_some_and(|a| a.visible),
            );
        }
    }

    pub(super) fn show_envelope(&mut self) -> (Option<String>, Vec<Correction>) {
        if self.envelope_widget.is_none() || self.pane.is_none() {
            return (Some("this window has no envelope body".into()), Vec::new());
        }
        self.ensure_envelope();
        self.contents.automation[0].visible = true;
        let mut corrections = self.envelope_corrections();
        corrections.extend(self.edit_layer("points").unwrap());
        corrections.push(Correction {
            widget: i64::from(self.pane.unwrap()),
            props: json!({"hidden":""}),
        });
        (None, corrections)
    }

    /// Application verbs use the same turn and record as host gestures.
    pub fn act(&mut self, request: &Value, version: i64) -> Outcome {
        let mut out = Outcome {
            turn: Kind::Route,
            version,
            ..Outcome::default()
        };
        let (reason, corrections) = match request.get("verb").and_then(Value::as_str).unwrap_or("")
        {
            "show_envelope" => self.show_envelope(),
            "setautomation" => {
                let flat = request
                    .get("points")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let values: Option<Vec<f64>> = flat.iter().map(Value::as_f64).collect();
                match values {
                    Some(values) => self.set_envelope(&values, &mut out),
                    None => (
                        Some("an envelope is numeric time/value/shape/curve quads".into()),
                        self.envelope_corrections(),
                    ),
                }
            }
            "preview_on" | "preview_off" | "remove_effect" => {
                self.preview_event(request["verb"].as_str().unwrap(), &mut out)
            }
            "apply_envelope" | "apply" => self.apply_envelope_event(&mut out),
            _ => (Some("unknown audio editor action".into()), Vec::new()),
        };
        out.answer = Some(if reason.is_some() && corrections.is_empty() {
            Answer::Ack {
                seq: 0,
                doc_version: out.version,
                reason,
            }
        } else {
            conversation::answer(0, out.version, reason, corrections)
        });
        out
    }

    pub(super) fn set_envelope_event(
        &mut self,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let mut flat: Vec<f64> = values.iter().map(number).collect();
        for quad in flat.as_chunks_mut::<4>().0 {
            quad[0] /= self.rate;
        }
        self.set_envelope(&flat, out)
    }

    fn set_envelope(
        &mut self,
        flat: &[f64],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if flat.is_empty() || !flat.len().is_multiple_of(4) || flat.iter().any(|v| !v.is_finite()) {
            return (
                Some("an envelope needs complete finite time/value/shape/curve quads".into()),
                self.envelope_corrections(),
            );
        }
        let mut flat = flat.to_vec();
        crate::points::Rules {
            values: Some((0.0, 2.0)),
            time: Some((0.0, self.length() as f64 / self.rate)),
        }
        .keep(&mut flat);
        if flat
            .as_chunks::<4>()
            .0
            .windows(2)
            .any(|p| p[0][0] >= p[1][0])
            || flat.as_chunks::<4>().0.iter().any(|p| {
                p[2].fract() != 0.0 || !(0.0..=8.0).contains(&p[2]) || !(p[3] as f32).is_finite()
            })
        {
            return (
                Some("envelope times must increase and shapes must be 0 through 8".into()),
                self.envelope_corrections(),
            );
        }
        let intake = points::intake("points", &flat);
        let points: Vec<clausters_document::Point<Second>> =
            serde_json::from_value(intake.payloads[0]["points"].clone()).unwrap();
        self.ensure_envelope();
        if points::same(&self.contents.automation[0].points, &points) {
            return (None, self.envelope_corrections());
        }
        let before = self.contents.clone();
        self.contents
            .set_automation(&MultitrackIntent::SetAutomation {
                automation: NodeId(1),
                points,
            })
            .unwrap();
        out.record = Some(self.record("set gain envelope", &before));
        out.changed = true;
        out.version += 1;
        (None, self.envelope_corrections())
    }

    pub(super) fn apply_envelope_event(
        &mut self,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        self.ensure_envelope();
        let curve = &self.contents.automation[0];
        if !curve.enabled || curve.points.iter().all(|p| p.value == 1.0) || self.length() == 0 {
            return (None, Vec::new());
        }
        let curve = curve.points.clone();
        let frames = self.length();
        // A cut keeps curves on the take's origin. Render a complete final
        // segment even if it now ends past the samples, then keep only the
        // take's span. Shortening a fade must not accelerate its shape.
        let rendered = curve.last().map_or(frames, |p| {
            frames.max(secs_to_samples(p.at.0, self.rate).max(0) as u64)
        });
        if rendered
            .checked_mul(u64::from(self.channels))
            .is_none_or(|n| n > i32::MAX as u64)
        {
            return (
                Some("the gain render exceeds the server buffer index range".into()),
                self.envelope_corrections(),
            );
        }
        let buffer = match self.next_buffer() {
            Ok(b) => b,
            Err(e) => return (Some(e), Vec::new()),
        };
        let mut steps = self.allocate(buffer, rendered);
        steps.push(send(
            "/buffer_gen",
            vec![
                OscType::Int(buffer as i32),
                OscType::String("copy".into()),
                OscType::Int(0),
                OscType::Int(self.display as i32),
                OscType::Int(0),
                OscType::Int((frames * u64::from(self.channels)) as i32),
            ],
        ));
        steps.push(Step::AwaitDone {
            command: "/buffer_gen".into(),
            index: Some(buffer as i32),
        });
        steps.extend(clausters_editing::gain::steps(
            buffer as i32,
            rendered,
            self.rate,
            &curve,
        ));
        let before = self.contents.clone();
        self.frames.insert(buffer, rendered);
        self.contents.parts = vec![part(buffer, 0, frames)];
        self.contents.automation[0].points = vec![
            points::point(Second(0.0), 1.0, 1.0, 0.0),
            points::point(Second(frames as f64 / self.rate), 1.0, 1.0, 0.0),
        ];
        steps.extend(self.stitched());
        out.record = Some(self.record("apply gain envelope", &before));
        out.steps = Some(steps_json(&steps));
        out.changed = true;
        out.version += 1;
        let mut corrections = self.widget.map_or_else(Vec::new, |w| resync(i64::from(w)));
        corrections.extend(self.envelope_corrections());
        (None, corrections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{Editing, Member};
    use clausters_document::history::Direction;

    fn editor() -> AudioEditor {
        let mut e = new_json(r#"{"take":1,"frames":48000,"display":2,"buffers":[3,4,5]}"#).unwrap();
        call_json(
            &mut e,
            r#"{"verb":"window","widget":10,"pane":11,"envelope":12}"#,
        );
        e
    }

    #[test]
    fn curve_body_and_gestures_use_shared_points_and_the_take_rate() {
        let mut e = editor();
        let tree: Value = serde_json::from_str(&call_json(
            &mut e,
            r#"{"verb":"window","widget":10,"pane":11,"envelope":12}"#,
        ))
        .unwrap();
        fn find(tree: &Value) -> Option<&Value> {
            if tree["id"] == 11 {
                return Some(tree);
            }
            tree["children"].as_array()?.iter().find_map(find)
        }
        let pane = find(&tree).unwrap();
        assert_eq!(pane["hidden"], "points");
        assert_eq!(pane["children"][1]["type"], "curve");
        assert_eq!(pane["children"][1]["points"][4], 48000.0);
        let show = e.act(&json!({"verb":"show_envelope"}), 1);
        assert!(!show.changed);
        assert_eq!(e.edit_layer, "points");
        let out = e.event(
            &Event {
                addr: "/gui_event".into(),
                args: vec![
                    json!(12),
                    json!(1),
                    json!(1),
                    json!("points"),
                    json!(0),
                    json!(0.5),
                    json!(5),
                    json!(-3),
                    json!(24000),
                    json!(1),
                    json!(1),
                    json!(0),
                ],
            },
            1,
        );
        assert!(out.changed);
        assert!(out.steps.is_none(), "a curve edit writes no samples");
        assert_eq!(
            e.envelope_quads(false),
            vec![0.0, 0.5, 5.0, -3.0, 0.5, 1.0, 1.0, 0.0]
        );
        assert_eq!(
            serde_json::to_value(out.answer.unwrap()).unwrap()["corrections"][0]["widget"],
            12
        );
    }

    #[test]
    fn edits_apply_and_undo_walk_one_order_without_samples_in_history() {
        let mut editing = Editing::default();
        let id = editing.join("take", Member::Audio(Box::new(editor())));
        let request = json!({"verb":"setautomation", "points":[0,0,1,0,1,1,1,0]});
        let first = editing.act(id, &request).unwrap();
        assert!(matches!(
            first.outcome,
            crate::editing::Outcome::Audio(Outcome { changed: true, .. })
        ));
        assert_eq!(editing.version(), 2);
        let applied = editing.act(id, &json!({"verb":"apply_envelope"})).unwrap();
        let crate::editing::Outcome::Audio(out) = applied.outcome else {
            panic!("audio");
        };
        assert!(out.changed);
        let steps = out.steps.unwrap();
        let wire = steps.to_string();
        assert!(wire.contains("/buffer_gain") && wire.contains("/buffer_gen"));
        assert!(!wire.contains("/buffer_render") && !wire.contains("/buffer_setRange"));
        assert!(
            out.record.unwrap().legs[0]
                .backward
                .get("automation")
                .is_some()
        );
        assert_eq!(editing.undo_label(), Some("apply gain envelope".into()));
        assert!(editing.step(Direction::Undo).stepped);
        let Some(Member::Audio(e)) = editing.member(id) else {
            panic!("audio");
        };
        assert_eq!(e.list()[0].source.source, SourceId(1));
        assert_eq!(e.envelope_quads(false)[1], 0.0);
        assert!(editing.step(Direction::Undo).stepped);
        let Some(Member::Audio(e)) = editing.member(id) else {
            panic!("audio");
        };
        assert_eq!(e.envelope_quads(false)[1], 1.0);
        assert!(editing.step(Direction::Redo).stepped);
        assert!(editing.step(Direction::Redo).stepped);
        let Some(Member::Audio(e)) = editing.member(id) else {
            panic!("audio");
        };
        assert_eq!(e.list()[0].source.source, SourceId(3));
        assert_eq!(e.envelope_quads(false)[1], 1.0);
    }

    #[test]
    fn unchanged_refused_and_unity_apply_do_not_consume_buffers_or_history() {
        let mut e = editor();
        let available = e.spare.clone();
        assert!(!e.act(&json!({"verb":"apply_envelope"}), 1).changed);
        let before = e.contents.clone();
        let out = e.act(&json!({"verb":"setautomation","points":[0,1,1]}), 1);
        assert!(serde_json::to_value(out.answer.unwrap()).unwrap()["reason"].is_string());
        assert_eq!(e.contents, before);
        assert_eq!(e.spare, available);
        let out = e.act(
            &json!({"verb":"setautomation","points":e.envelope_quads(false)}),
            1,
        );
        assert!(!out.changed);
        assert!(out.record.is_none());
        e.act(&json!({"verb":"setautomation","points":[0,0.5,1,0]}), 1);
        e.spare.clear();
        let before = e.contents.clone();
        assert!(
            serde_json::to_value(e.act(&json!({"verb":"apply_envelope"}), 2).answer.unwrap())
                .unwrap()["reason"]
                .is_string()
        );
        assert_eq!(e.contents, before);
    }
}
