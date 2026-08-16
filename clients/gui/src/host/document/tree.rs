//! Drawing a document as a multitrack — the host's own `Editor`.
//!
//! The Python client has one and it is the reference; this is the port a
//! standalone host needs, because a host with no language client still has to
//! show what it holds. It is deliberately the **same picture**: lanes of clips
//! over one shared time axis, a beat ruler under them, ids allocated as it goes
//! and every clip bound to the node it draws.
//!
//! # It builds a GuiDef, and nothing else
//!
//! What comes out is the ordinary `{id, type, props, children}` tree the host
//! already parses — no new widget, no new prop, no path into the tree that a
//! script could not take. That is the point: a standalone editor is this host
//! driven by itself, so anything it can draw a script can draw too, and
//! anything it cannot is missing for both.
//!
//! # What it does not draw yet
//!
//! A clip's **body** — the take, the notes, the automation curve under the
//! label — needs the session's sources resolved to files and buffers, which is
//! the half that belongs with the session and not with the tree. Until then a
//! clip is its placement and its name, which is enough to move it, undo it and
//! save it, and honest about the rest.

use clausters_document::{Beats, Body, Document, Member, Node, NodeId};
use serde_json::{Map, Value, json};

/// One clip or lane the tree drew, and the node it draws.
///
/// The binding is the whole reason this returns anything besides JSON: an
/// intent names a node, a gesture names a widget, and only what built the tree
/// knows which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    pub widget: i32,
    pub node: NodeId,
}

/// How the picture is scaled and labelled.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    /// Samples per beat — the unit a clip's `offset`/`dur` are in, since the
    /// shared time axis measures samples and the document measures beats.
    pub units_per_beat: f64,
    /// The rate the ruler names its ticks in.
    pub sample_rate: f64,
    /// Beats per second, for the ruler's bar and beat lines.
    pub tempo: f64,
    /// The grid a placement snaps to, in beats; 0 snaps nothing.
    pub quant: Beats,
    /// The first widget id to allocate. Ids are the host's own namespace, and a
    /// caller that already used some says where to carry on from.
    pub first_id: i32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            units_per_beat: 48_000.0,
            sample_rate: 48_000.0,
            tempo: 1.0,
            quant: 0.0,
            first_id: 1,
        }
    }
}

/// The window a document draws as, plus what each widget in it is a picture of.
pub struct Drawn {
    /// The GuiDef, ready for `/gui_def`.
    pub def: Value,
    /// Every clip's widget, and the node it draws.
    pub bindings: Vec<Bound>,
    /// The next free widget id, so a caller can keep allocating after it.
    pub next_id: i32,
}

/// Draws `document` as a window of lanes.
///
/// One lane per top-level member: a **set** becomes a lane of its members'
/// clips (which is what a track is), and anything else becomes a lane holding
/// one clip. Nesting deeper than that is drawn flat for now — a set inside a
/// set is one lane of its own, in document order — because an expanded/collapsed
/// state is a thing the *editor* holds and this has nowhere yet to keep one.
pub fn draw(document: &Document, look: &Look, title: &str) -> Drawn {
    let mut ids = Ids {
        next: look.first_id,
    };
    let mut bindings = Vec::new();
    let mut lanes: Vec<Value> = Vec::new();

    match &document.root.body {
        Body::Set { members, .. } => {
            for member in members {
                lane_of(member, look, &mut ids, &mut bindings, &mut lanes);
            }
        }
        // A document that is one thing is one lane holding it.
        _ => {
            let member = Member {
                offset: 0.0,
                dur: None,
                node: document.root.clone(),
            };
            lane_of(&member, look, &mut ids, &mut bindings, &mut lanes);
        }
    }

    // The ruler joins the lanes' navigation group rather than owning a strip
    // inside one of them, exactly as the Python editor places it.
    lanes.push(json!({
        "id": ids.take(),
        "type": "timeruler",
        "ruler": "beats",
        "sample_rate": look.sample_rate,
        "tempo": look.tempo,
    }));

    let def = json!({
        "type": "window",
        "title": title,
        "layout": "col",
        "w": 1000,
        "h": 640,
        "children": lanes,
    });
    Drawn {
        def,
        bindings,
        next_id: ids.next,
    }
}

struct Ids {
    next: i32,
}

impl Ids {
    fn take(&mut self) -> i32 {
        let id = self.next;
        self.next += 1;
        id
    }
}

fn lane_of(
    member: &Member,
    look: &Look,
    ids: &mut Ids,
    bindings: &mut Vec<Bound>,
    lanes: &mut Vec<Value>,
) {
    let label = label_of(&member.node);
    let clips = match &member.node.body {
        Body::Set { members, .. } => members
            .iter()
            // A set's members are placed relative to it, and a clip's offset is
            // absolute on the shared axis: the two are added here, once.
            .map(|inner| clip_of(inner, member.offset, look, ids, bindings))
            .collect(),
        _ => vec![clip_of(member, 0.0, look, ids, bindings)],
    };
    let mut props = Map::new();
    props.insert("id".into(), json!(ids.take()));
    props.insert("type".into(), json!("track"));
    props.insert("label".into(), json!(label));
    props.insert("sample_rate".into(), json!(look.sample_rate));
    props.insert("tempo".into(), json!(look.tempo));
    if look.quant > 0.0 {
        props.insert("snap".into(), json!(look.quant * look.units_per_beat));
    }
    props.insert("children".into(), Value::Array(clips));
    lanes.push(Value::Object(props));
}

fn clip_of(
    member: &Member,
    base: Beats,
    look: &Look,
    ids: &mut Ids,
    bindings: &mut Vec<Bound>,
) -> Value {
    let widget = ids.take();
    bindings.push(Bound {
        widget,
        node: member.node.id,
    });
    // The length shown: the placement's where it overrides, else the element's
    // own, else a beat — a clip with no length at all would be a line.
    let dur = member
        .dur
        .or(member.node.duration)
        .filter(|d| *d > 0.0)
        .unwrap_or(1.0);
    json!({
        "id": widget,
        "type": "clip",
        "offset": (base + member.offset) * look.units_per_beat,
        "dur": dur * look.units_per_beat,
        "label": label_of(&member.node),
    })
}

/// What a node is called on screen. The document holds no names — a name is a
/// client's idea — so this says what it *is*, which is what a reader needs from
/// a picture drawn by a host that was handed a file.
fn label_of(node: &Node) -> String {
    match &node.body {
        Body::Event { .. } => format!("event {}", node.id.0),
        Body::Sequence { .. } => format!("sequence {}", node.id.0),
        Body::Buffer { .. } => format!("take {}", node.id.0),
        Body::Set { grouping, .. } => format!("{grouping:?} {}", node.id.0).to_lowercase(),
        Body::Generator { .. } => format!("generator {}", node.id.0),
        // A body this build does not know: drawn as what it is rather than
        // refused, which is the same courtesy an older host shows a newer
        // widget. A document written by something ahead of us still opens, and
        // what it holds is still moved, undone and saved unchanged.
        Body::Unknown(_) => format!("node {}", node.id.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::{Grouping, Opaque};

    fn event(id: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Event {
                config: Opaque::default(),
                fires: None,
            },
        )
    }

    fn placed(offset: Beats, dur: Option<Beats>, node: Node) -> Member {
        Member { offset, dur, node }
    }

    fn set(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Set {
                grouping: Grouping::Concrete,
                members,
            },
        )
    }

    fn children(def: &Value) -> &Vec<Value> {
        def["children"].as_array().expect("a window has children")
    }

    #[test]
    fn a_set_of_sets_draws_one_lane_each_and_a_ruler_under_them() {
        let doc = Document::new(set(
            1,
            vec![
                placed(0.0, None, set(2, vec![placed(0.0, Some(2.0), event(3))])),
                placed(0.0, None, set(4, vec![placed(1.0, Some(1.0), event(5))])),
            ],
        ));
        let drawn = draw(&doc, &Look::default(), "session");
        let kids = children(&drawn.def);
        assert_eq!(kids.len(), 3, "two lanes and the ruler");
        assert_eq!(kids[0]["type"], "track");
        assert_eq!(kids[1]["type"], "track");
        assert_eq!(kids[2]["type"], "timeruler");
    }

    /// A clip's offset is **absolute on the shared axis** while a member's is
    /// relative to its set: the two are added once, here, or every lane after
    /// the first would draw in the wrong place.
    #[test]
    fn a_nested_placement_is_absolute_on_the_shared_axis() {
        let doc = Document::new(set(
            1,
            vec![placed(
                4.0, // the lane starts at beat 4
                None,
                set(2, vec![placed(1.0, Some(2.0), event(3))]), // the clip at 1 within it
            )],
        ));
        let look = Look {
            units_per_beat: 100.0,
            ..Look::default()
        };
        let drawn = draw(&doc, &look, "session");
        let clip = &children(&drawn.def)[0]["children"][0];
        assert_eq!(clip["offset"], 500.0, "4 + 1 beats, in units");
        assert_eq!(clip["dur"], 200.0);
    }

    /// Every clip says which node it draws — the one thing only what built the
    /// tree can know, and what an intent needs to name.
    #[test]
    fn every_clip_is_bound_to_the_node_it_draws() {
        let doc = Document::new(set(
            1,
            vec![placed(
                0.0,
                None,
                set(
                    2,
                    vec![placed(0.0, None, event(7)), placed(1.0, None, event(8))],
                ),
            )],
        ));
        let drawn = draw(&doc, &Look::default(), "session");
        let nodes: Vec<u64> = drawn.bindings.iter().map(|b| b.node.0).collect();
        assert_eq!(nodes, vec![7, 8], "the clips, in document order");
        let clips = children(&drawn.def)[0]["children"].as_array().unwrap();
        let widgets: Vec<i64> = clips.iter().map(|c| c["id"].as_i64().unwrap()).collect();
        assert_eq!(
            widgets,
            drawn
                .bindings
                .iter()
                .map(|b| b.widget as i64)
                .collect::<Vec<_>>(),
            "and the bindings name those very widgets"
        );
        assert!(drawn.next_id > widgets.iter().max().copied().unwrap() as i32);
    }

    /// A document that is one thing is still a window: a host handed a file
    /// draws what it was given rather than refusing a shape.
    #[test]
    fn a_document_that_is_not_a_set_still_draws() {
        let doc = Document::new(event(1));
        let drawn = draw(&doc, &Look::default(), "one thing");
        let kids = children(&drawn.def);
        assert_eq!(kids.len(), 2, "one lane and the ruler");
        assert_eq!(drawn.bindings.len(), 1);
        assert_eq!(drawn.bindings[0].node, NodeId(1));
    }

    #[test]
    fn a_grid_reaches_the_lane_and_nothing_reaches_it_when_there_is_none() {
        let doc = Document::new(set(1, vec![placed(0.0, None, event(2))]));
        let plain = draw(&doc, &Look::default(), "t");
        assert!(children(&plain.def)[0].get("snap").is_none());
        let snapped = draw(
            &doc,
            &Look {
                quant: 0.5,
                units_per_beat: 100.0,
                ..Look::default()
            },
            "t",
        );
        assert_eq!(children(&snapped.def)[0]["snap"], 50.0);
    }
}
