//! The live node-tree view: mirror the audio server's node tree, draw it.
//!
//! A read-only view that exercises the host's *client of the audio server* leg.
//! The host queries the server's tree with `/g_queryTree <group> 1` over the
//! client leg ([`super::client`]) and receives a `/g_queryTree.reply`; this
//! module turns that flat reply into a small [`NodeTree`] model (pure and
//! testable, no OSC and no GPU) and draws it as indented text through the
//! flat-geometry painter ([`super::paint`]) plus bitmap text — the same cheap
//! path the meters and scopes use, no dedicated pipeline.
//!
//! The reply is scsynth's depth-first encoding (mirrored by the server's
//! `CmdTranslator::query_tree`): `flag`, the queried group id and its child
//! count, then per node its id and child count (`-1` marks a synth), a synth's
//! def name and — with `flag` — its control count followed by `(name|index,
//! value)` pairs, a group's children inline. Parsing tolerates a short or
//! malformed reply by returning `None` rather than panicking.

use clausters_core::osc::OscType;

use super::controls::body_rect;
use super::font;
use super::layout::Rect;
use super::paint::{Color, Mesh};

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const DIM: Color = [0.55, 0.60, 0.66, 1.0];
const FIELD: Color = [0.10, 0.11, 0.14, 1.0];
const FRAME: Color = [0.30, 0.45, 0.60, 1.0];
const PAD: f32 = 4.0;
const TEXT_SCALE: f32 = 2.0;
/// Pixels a child is indented past its parent.
const INDENT: f32 = 14.0;

/// One node of the mirrored tree: its id and whether it is a group (with its
/// children) or a synth (with its def name and control values).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeEntry {
    pub id: i32,
    pub body: NodeBody,
}

/// A node's contents: a group holds its children, a synth its def and controls.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeBody {
    Group(Vec<NodeEntry>),
    Synth {
        def_name: String,
        controls: Vec<(String, f32)>,
    },
}

/// The node tree rooted at one group, as last read from the server.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeTree {
    /// The group this tree was queried for.
    pub group: i32,
    /// The direct children of `group` (groups and synths), depth-first.
    pub root: Vec<NodeEntry>,
}

/// One rendered line: its indentation depth and text.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub depth: usize,
    pub text: String,
}

impl NodeTree {
    /// Parses a `/g_queryTree.reply` argument list into a tree, or `None` if the
    /// reply is short or malformed (a corrupt reply must not panic the host).
    pub fn parse(args: &[OscType]) -> Option<NodeTree> {
        let mut it = args.iter();
        let with_controls = next_int(&mut it)? != 0;
        let group = next_int(&mut it)?;
        let count = next_int(&mut it)?.max(0) as usize;
        let root = parse_children(&mut it, count, with_controls)?;
        Some(NodeTree { group, root })
    }

    /// The flattened display lines (depth + text), header group first. When
    /// `controls`, each synth's control name/value pairs follow it, indented.
    pub fn lines(&self, controls: bool) -> Vec<Line> {
        let mut out = vec![Line {
            depth: 0,
            text: format!("group {}", self.group),
        }];
        for entry in &self.root {
            push_lines(entry, 1, controls, &mut out);
        }
        out
    }
}

fn parse_children<'a>(
    it: &mut impl Iterator<Item = &'a OscType>,
    count: usize,
    with_controls: bool,
) -> Option<Vec<NodeEntry>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let id = next_int(it)?;
        let child_count = next_int(it)?;
        let body = if child_count < 0 {
            // A synth: def name, then (with the flag) its controls.
            let def_name = next_string(it)?;
            let mut controls = Vec::new();
            if with_controls {
                let n = next_int(it)?.max(0) as usize;
                for _ in 0..n {
                    let name = next_control_name(it)?;
                    let value = next_float(it)?;
                    controls.push((name, value));
                }
            }
            NodeBody::Synth { def_name, controls }
        } else {
            NodeBody::Group(parse_children(it, child_count as usize, with_controls)?)
        };
        out.push(NodeEntry { id, body });
    }
    Some(out)
}

fn push_lines(entry: &NodeEntry, depth: usize, controls: bool, out: &mut Vec<Line>) {
    match &entry.body {
        NodeBody::Group(children) => {
            out.push(Line {
                depth,
                text: format!("{} group", entry.id),
            });
            for child in children {
                push_lines(child, depth + 1, controls, out);
            }
        }
        NodeBody::Synth {
            def_name,
            controls: ctrls,
        } => {
            out.push(Line {
                depth,
                text: format!("{} {def_name}", entry.id),
            });
            if controls {
                for (name, value) in ctrls {
                    out.push(Line {
                        depth: depth + 1,
                        text: format!("{name} {}", fmt(*value)),
                    });
                }
            }
        }
    }
}

/// Draws the node tree into `rect`: a framed field with a label strip, then the
/// indented lines (clipped to the body height). `tree` is `None` before the
/// first reply; `server` reports whether a client leg is attached at all, so an
/// unattached host shows why it is empty instead of looking broken.
pub fn draw(
    mesh: &mut Mesh,
    rect: Rect,
    tree: Option<&NodeTree>,
    controls: bool,
    label: Option<&str>,
    server: bool,
) {
    if let Some(text) = label {
        font::text(mesh, text, rect.x + PAD, rect.y + PAD, TEXT_SCALE, TEXT);
    }
    let body = body_rect(rect, label.is_some());
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, FIELD);
    mesh.border(body, 1.0, FRAME);

    let line_h = font::height(TEXT_SCALE) + 3.0;
    match tree {
        None => placeholder(mesh, body, if server { "querying..." } else { "no server" }),
        Some(tree) => {
            let lines = tree.lines(controls);
            let mut y = body.y + PAD;
            for line in &lines {
                if y + font::height(TEXT_SCALE) > body.y + body.h {
                    break; // out of vertical room; scrolling is future work
                }
                let x = body.x + PAD + line.depth as f32 * INDENT;
                font::text(mesh, &line.text, x, y, TEXT_SCALE, TEXT);
                y += line_h;
            }
        }
    }
}

/// A dim centered note for the empty states.
fn placeholder(mesh: &mut Mesh, body: Rect, text: &str) {
    font::text_centered(mesh, text, body, TEXT_SCALE, DIM);
}

fn next_int<'a>(it: &mut impl Iterator<Item = &'a OscType>) -> Option<i32> {
    match it.next()? {
        OscType::Int(n) => Some(*n),
        OscType::Long(n) => Some(*n as i32),
        _ => None,
    }
}

fn next_float<'a>(it: &mut impl Iterator<Item = &'a OscType>) -> Option<f32> {
    match it.next()? {
        OscType::Float(x) => Some(*x),
        OscType::Double(x) => Some(*x as f32),
        OscType::Int(n) => Some(*n as f32),
        _ => None,
    }
}

fn next_string<'a>(it: &mut impl Iterator<Item = &'a OscType>) -> Option<String> {
    match it.next()? {
        OscType::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A control identifier: a name string, or an integer index when the server has
/// no name for it (it sends the index instead). Rendered as `#<index>`.
fn next_control_name<'a>(it: &mut impl Iterator<Item = &'a OscType>) -> Option<String> {
    match it.next()? {
        OscType::String(s) => Some(s.clone()),
        OscType::Int(n) => Some(format!("#{n}")),
        _ => None,
    }
}

/// Formats a control value compactly (drops trailing zeros within 3 decimals).
fn fmt(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        return format!("{v:.0}");
    }
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply for: group 0 holding group 1, which holds synth 1000 "sine"
    /// (controls freq=440, amp=0.2). Built the way the server encodes it.
    fn sample_reply() -> Vec<OscType> {
        vec![
            OscType::Int(1), // flag: controls included
            OscType::Int(0), // queried group
            OscType::Int(1), // it has one child
            // group 1, one child
            OscType::Int(1),
            OscType::Int(1),
            // synth 1000, -1 marks a synth, def "sine", 2 controls
            OscType::Int(1000),
            OscType::Int(-1),
            OscType::String("sine".into()),
            OscType::Int(2),
            OscType::String("freq".into()),
            OscType::Float(440.0),
            OscType::String("amp".into()),
            OscType::Float(0.2),
        ]
    }

    #[test]
    fn parses_nested_groups_and_synth_controls() {
        let tree = NodeTree::parse(&sample_reply()).expect("valid reply parses");
        assert_eq!(tree.group, 0);
        assert_eq!(tree.root.len(), 1);
        let NodeBody::Group(children) = &tree.root[0].body else {
            panic!("expected a group at the root");
        };
        assert_eq!(tree.root[0].id, 1);
        assert_eq!(children.len(), 1);
        match &children[0].body {
            NodeBody::Synth { def_name, controls } => {
                assert_eq!(def_name, "sine");
                assert_eq!(controls.len(), 2);
                assert_eq!(controls[0], ("freq".into(), 440.0));
                assert_eq!(controls[1], ("amp".into(), 0.2));
            }
            other => panic!("expected a synth, got {other:?}"),
        }
    }

    #[test]
    fn lines_indent_by_depth_and_show_controls() {
        let tree = NodeTree::parse(&sample_reply()).unwrap();
        let lines = tree.lines(true);
        // group 0 (d0) > group 1 (d1) > sine (d2) > freq (d3) > amp (d3).
        let depths: Vec<usize> = lines.iter().map(|l| l.depth).collect();
        assert_eq!(depths, vec![0, 1, 2, 3, 3]);
        assert_eq!(lines[0].text, "group 0");
        assert_eq!(lines[2].text, "1000 sine");
        assert_eq!(lines[3].text, "freq 440");
        // Without controls, the synth's parameter lines are dropped.
        assert_eq!(tree.lines(false).len(), 3);
    }

    #[test]
    fn an_empty_tree_is_just_the_header() {
        let reply = vec![OscType::Int(0), OscType::Int(0), OscType::Int(0)];
        let tree = NodeTree::parse(&reply).unwrap();
        assert!(tree.root.is_empty());
        assert_eq!(
            tree.lines(true),
            vec![Line {
                depth: 0,
                text: "group 0".into()
            }]
        );
    }

    #[test]
    fn an_index_control_renders_with_a_hash() {
        // A control with no name comes back as an integer index.
        let reply = vec![
            OscType::Int(1),
            OscType::Int(0),
            OscType::Int(1),
            OscType::Int(7),
            OscType::Int(-1),
            OscType::String("anon".into()),
            OscType::Int(1),
            OscType::Int(3),
            OscType::Float(1.5),
        ];
        let tree = NodeTree::parse(&reply).unwrap();
        let lines = tree.lines(true);
        assert_eq!(lines[2].text, "#3 1.5");
    }

    #[test]
    fn a_truncated_reply_returns_none_not_a_panic() {
        // Claims a child but ends early.
        let reply = vec![OscType::Int(0), OscType::Int(0), OscType::Int(1)];
        assert!(NodeTree::parse(&reply).is_none());
    }

    #[test]
    fn draw_emits_geometry_for_a_tree() {
        let tree = NodeTree::parse(&sample_reply()).unwrap();
        let mut m = Mesh::new();
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 200.0, 200.0),
            Some(&tree),
            true,
            Some("tree"),
            true,
        );
        assert!(!m.is_empty(), "a populated node tree draws geometry");
    }
}
