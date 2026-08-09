//! The live node-tree view: mirror the audio server's node tree, draw it.
//!
//! A read-only view that exercises the host's *client of the audio server* leg.
//! The host queries the server's tree with `/group_queryTree <group> 1` over the
//! client leg ([`super::client`]) and receives a `/group_queryTree.reply`; this
//! module turns that flat reply into a small [`NodeTree`] model (pure and
//! testable, no OSC and no GPU) and draws it as indented text through the
//! flat-geometry painter ([`super::paint`]) plus bitmap text — the same cheap
//! path the meters and scopes use, no dedicated pipeline.
//!
//! The reply is the server's depth-first encoding (`CmdTranslator::query_tree`):
//! a **detail level**, the queried group id, its child count and its name, then
//! per node its id, its child count (`-1` marks a synth) and a name — a group's
//! own (empty when it has none) or a synth's def name — and, for a synth from
//! detail 1, its control count followed by `(name|index, value)` pairs; a
//! group's children follow inline. Every node reads `id, count, name`, one
//! shape for both kinds, which is what keeps the walk in step. Detail 2 appends
//! what a full node info carries (maps, inferred bus lists), which this view
//! does not draw and skips; the host asks for 1. Parsing tolerates a short or
//! malformed reply by returning `None` rather than panicking.

use clausters_core::osc::OscType;

use super::controls::body_rect;
use super::font;
use super::layout::Rect;
use super::metrics::Metrics;
use super::paint::{Draw, Mesh};
use super::theme::Theme;

/// One node of the mirrored tree: its id and whether it is a group (with its
/// children) or a synth (with its def name and control values).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeEntry {
    pub id: i32,
    pub body: NodeBody,
}

/// A node's contents: a group holds its name and its children, a synth its def
/// and controls. A group's name is empty when it has none, and the id stays its
/// identity either way.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeBody {
    Group {
        name: String,
        children: Vec<NodeEntry>,
    },
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
    /// That group's own name, empty when it has none.
    pub name: String,
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
    /// Parses a `/group_queryTree.reply` argument list into a tree, or `None` if the
    /// reply is short or malformed (a corrupt reply must not panic the host).
    pub fn parse(args: &[OscType]) -> Option<NodeTree> {
        let mut it = args.iter();
        let detail = next_int(&mut it)?;
        let group = next_int(&mut it)?;
        let count = next_int(&mut it)?.max(0) as usize;
        let name = next_string(&mut it)?;
        let root = parse_children(&mut it, count, detail)?;
        Some(NodeTree { group, name, root })
    }

    /// The flattened display lines (depth + text), header group first. When
    /// `controls`, each synth's control name/value pairs follow it, indented.
    pub fn lines(&self, controls: bool) -> Vec<Line> {
        let mut out = vec![Line {
            depth: 0,
            text: group_text(self.group, &self.name),
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
    detail: i32,
) -> Option<Vec<NodeEntry>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let id = next_int(it)?;
        let child_count = next_int(it)?;
        // Every node names itself here, group or synth; a group's name is empty
        // when it has none. Reading it for both is what keeps the walk aligned.
        let name = next_string(it)?;
        let body = if child_count < 0 {
            // A synth: the name was its def, then its controls from detail 1.
            let def_name = name;
            let mut controls = Vec::new();
            if detail >= 1 {
                let n = next_int(it)?.max(0) as usize;
                for _ in 0..n {
                    let name = next_control_name(it)?;
                    let value = next_float(it)?;
                    controls.push((name, value));
                }
            }
            if detail >= 2 {
                // The full-record tail this view does not draw: the maps as
                // (control, bus, audio) triples, then reads/writes.
                let n = next_int(it)?.max(0) as usize;
                for _ in 0..n * 3 {
                    next_int(it)?;
                }
                next_string(it)?;
                next_string(it)?;
            }
            NodeBody::Synth { def_name, controls }
        } else {
            NodeBody::Group {
                name,
                children: parse_children(it, child_count as usize, detail)?,
            }
        };
        out.push(NodeEntry { id, body });
    }
    Some(out)
}

/// The header line for the queried group: its id, and its name when it has one.
fn group_text(id: i32, name: &str) -> String {
    if name.is_empty() {
        format!("group {id}")
    } else {
        format!("group {id} {name}")
    }
}

fn push_lines(entry: &NodeEntry, depth: usize, controls: bool, out: &mut Vec<Line>) {
    match &entry.body {
        NodeBody::Group { name, children } => {
            out.push(Line {
                depth,
                text: if name.is_empty() {
                    format!("{} group", entry.id)
                } else {
                    format!("{} {name}", entry.id)
                },
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
    d: &mut Draw,
    rect: Rect,
    tree: Option<&NodeTree>,
    controls: bool,
    label: Option<&str>,
    server: bool,
) {
    let (mesh, m, theme) = d.parts();
    if let Some(text) = label {
        font::text(
            mesh,
            text,
            rect.x + m.pad,
            rect.y + m.pad,
            m.text_scale,
            theme.text,
        );
    }
    let body = body_rect(rect, label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.track);
    mesh.border(body, m.divider_w, theme.frame_info);

    let line_h = font::height(m.text_scale) + 3.0;
    match tree {
        None => placeholder(
            mesh,
            body,
            if server { "querying..." } else { "no server" },
            m,
            theme,
        ),
        Some(tree) => {
            let lines = tree.lines(controls);
            let mut y = body.y + m.pad;
            for line in &lines {
                if y + font::height(m.text_scale) > body.y + body.h {
                    break; // out of vertical room; scrolling is future work
                }
                let x = body.x + m.pad + line.depth as f32 * m.indent;
                font::text(mesh, &line.text, x, y, m.text_scale, theme.text);
                y += line_h;
            }
        }
    }
}

/// A dim centered note for the empty states.
fn placeholder(mesh: &mut Mesh, body: Rect, text: &str, m: &Metrics, theme: &Theme) {
    font::text_centered(mesh, text, body, m.text_scale, theme.text_dim);
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

    /// A reply for: group 0 (unnamed) holding group 1 "mixer", which holds synth
    /// 1000 "sine" (controls freq=440, amp=0.2). Built the way the server
    /// encodes it — every node reads `id, count, name`, groups included.
    fn sample_reply() -> Vec<OscType> {
        vec![
            OscType::Int(1),            // flag: controls included
            OscType::Int(0),            // queried group
            OscType::Int(1),            // it has one child
            OscType::String("".into()), // and no name of its own
            // group 1 "mixer", one child
            OscType::Int(1),
            OscType::Int(1),
            OscType::String("mixer".into()),
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
        assert_eq!(tree.name, "");
        let NodeBody::Group { name, children } = &tree.root[0].body else {
            panic!("expected a group at the root");
        };
        assert_eq!(tree.root[0].id, 1);
        assert_eq!(name, "mixer");
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
        // A named group reads like a synth does: the id, then what it is called.
        assert_eq!(lines[1].text, "1 mixer");
        assert_eq!(lines[2].text, "1000 sine");
        assert_eq!(lines[3].text, "freq 440");
        // Without controls, the synth's parameter lines are dropped.
        assert_eq!(tree.lines(false).len(), 3);
    }

    #[test]
    fn an_empty_tree_is_just_the_header() {
        let reply = vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("".into()),
        ];
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
    fn the_queried_group_shows_its_own_name() {
        let reply = vec![
            OscType::Int(0),
            OscType::Int(1000),
            OscType::Int(0),
            OscType::String("console".into()),
        ];
        let tree = NodeTree::parse(&reply).unwrap();
        assert_eq!(tree.name, "console");
        assert_eq!(tree.lines(true)[0].text, "group 1000 console");
    }

    #[test]
    fn an_index_control_renders_with_a_hash() {
        // A control with no name comes back as an integer index.
        let reply = vec![
            OscType::Int(1),
            OscType::Int(0),
            OscType::Int(1),
            OscType::String("".into()),
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
        let reply = vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(1),
            OscType::String("".into()),
        ];
        assert!(NodeTree::parse(&reply).is_none());
    }

    #[test]
    fn draw_emits_geometry_for_a_tree() {
        let tree = NodeTree::parse(&sample_reply()).unwrap();
        let mut m = Mesh::new();
        draw(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 200.0, 200.0),
            Some(&tree),
            true,
            Some("tree"),
            true,
        );
        assert!(!m.is_empty(), "a populated node tree draws geometry");
    }
}
