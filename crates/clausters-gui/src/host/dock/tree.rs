//! **The dock's layout as data**: a tree of splits whose leaves are groups of
//! tabs, the moves that rearrange it, and the JSON it is saved as.
//!
//! A split lays its parts side by side (a row) or one under another (a
//! column), each with its share of the room; a group shows one of its panes
//! and a tab for each. Nothing here knows a pixel: [`super::Dock`] measures
//! the tree into rectangles, and the moves are the same whether a hand or a
//! script asked for them.
//!
//! The tree is kept **normal** after every move, so two layouts that look the
//! same are the same data: no group is empty (but the root's, in a dock with
//! no pane), no split has fewer than two parts, a split never holds a split
//! of its own axis (its parts join the parent's), and the shares of a split
//! sum to one.

use serde_json::{Value, json};

/// Which side of a group a pane is put on, making a split there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// The side a word names: `left`, `right`, `top`, `bottom`.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "left" => Self::Left,
            "right" => Self::Right,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => return None,
        })
    }

    /// Whether the split it makes is a row (the parts side by side).
    pub fn row(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// Whether the new part goes after the group it splits.
    fn after(self) -> bool {
        matches!(self, Self::Right | Self::Bottom)
    }
}

/// A group of tabs: its panes in the order of their tabs, the one shown,
/// and whether it is **folded** to its bar.
///
/// A group folds only where it has someone to give its room to: in a column
/// with a part that stays open. Folded, it is its bar tall, the open parts
/// of its column share what it gave up, and its panes are not shown, as a
/// pane behind another's tab is not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub panes: Vec<i32>,
    pub active: usize,
    pub folded: bool,
}

impl Group {
    fn of(pane: i32) -> Self {
        Self {
            panes: vec![pane],
            ..Self::default()
        }
    }

    /// The pane it shows.
    pub fn shown(&self) -> Option<i32> {
        self.panes.get(self.active).copied()
    }
}

/// One node of the layout.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// Parts side by side (`row`) or one under another, each with its share
    /// of the room.
    Split { row: bool, parts: Vec<(f32, Node)> },
    /// A group of tabs.
    Tabs(Group),
}

/// Where a node is: the index of the part taken at each split from the root.
pub type Path = Vec<usize>;

impl Default for Node {
    fn default() -> Self {
        Node::Tabs(Group::default())
    }
}

impl Node {
    /// The node at `path`.
    pub fn at(&self, path: &[usize]) -> Option<&Node> {
        match (path.split_first(), self) {
            (None, _) => Some(self),
            (Some((&i, rest)), Node::Split { parts, .. }) => parts.get(i)?.1.at(rest),
            _ => None,
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((&i, rest)) => match self {
                Node::Split { parts, .. } => parts.get_mut(i)?.1.at_mut(rest),
                Node::Tabs(_) => None,
            },
        }
    }

    /// Every pane, groups in the order they are laid out, tabs in theirs.
    pub fn panes(&self) -> Vec<i32> {
        let mut out = Vec::new();
        self.walk(&mut |_, g| out.extend(&g.panes));
        out
    }

    /// Calls `f` with every group and its path, in layout order.
    pub fn walk(&self, f: &mut dyn FnMut(&Path, &Group)) {
        fn go(node: &Node, path: &mut Path, f: &mut dyn FnMut(&Path, &Group)) {
            match node {
                Node::Tabs(g) => f(path, g),
                Node::Split { parts, .. } => {
                    for (i, (_, part)) in parts.iter().enumerate() {
                        path.push(i);
                        go(part, path, f);
                        path.pop();
                    }
                }
            }
        }
        go(self, &mut Vec::new(), f);
    }

    /// The path of the group holding `pane`.
    pub fn group_of(&self, pane: i32) -> Option<Path> {
        let mut found = None;
        self.walk(&mut |path, g| {
            if found.is_none() && g.panes.contains(&pane) {
                found = Some(path.clone());
            }
        });
        found
    }

    /// The group at `path`.
    pub fn group(&self, path: &[usize]) -> Option<&Group> {
        match self.at(path)? {
            Node::Tabs(g) => Some(g),
            Node::Split { .. } => None,
        }
    }

    fn group_mut(&mut self, path: &[usize]) -> Option<&mut Group> {
        match self.at_mut(path)? {
            Node::Tabs(g) => Some(g),
            Node::Split { .. } => None,
        }
    }

    /// **A new pane, as a column of its own** at the right of everything:
    /// a dock of columns stays a dock of columns, each as wide as the others
    /// if they were.
    pub fn add(&mut self, pane: i32) {
        match self {
            Node::Tabs(g) if g.panes.is_empty() => *g = Group::of(pane),
            Node::Split { row: true, parts } => {
                let n = parts.len() as f32;
                for (share, _) in parts.iter_mut() {
                    *share *= n / (n + 1.0);
                }
                parts.push((1.0 / (n + 1.0), Node::Tabs(Group::of(pane))));
            }
            _ => {
                let old = std::mem::take(self);
                *self = Node::Split {
                    row: true,
                    parts: vec![(0.5, old), (0.5, Node::Tabs(Group::of(pane)))],
                };
            }
        }
    }

    /// **Takes `pane` out**; a group it leaves empty goes, and its room goes
    /// to the parts beside it. Answers whether it was here.
    pub fn remove(&mut self, pane: i32) -> bool {
        let Some(path) = self.group_of(pane) else {
            return false;
        };
        if let Some(g) = self.group_mut(&path) {
            let at = g.panes.iter().position(|&p| p == pane).unwrap_or(0);
            g.panes.remove(at);
            if at < g.active || g.active >= g.panes.len() {
                g.active = g.active.saturating_sub(1);
            }
        }
        self.normalize();
        true
    }

    /// **Shows `pane`**, in the group that holds it. Answers whether that
    /// changed what is shown.
    pub fn select(&mut self, pane: i32) -> bool {
        let Some(path) = self.group_of(pane) else {
            return false;
        };
        let Some(g) = self.group_mut(&path) else {
            return false;
        };
        let at = g.panes.iter().position(|&p| p == pane).unwrap_or(0);
        // a pane shown is seen: its group unfolds
        let changed = g.active != at || g.folded;
        g.active = at;
        g.folded = false;
        changed
    }

    /// **Folds the group at `path` to its bar, or unfolds it.** Answers
    /// whether that changed anything: a group that may not fold
    /// ([`foldable`](Self::foldable)) stays open.
    pub fn fold(&mut self, path: &[usize], folded: bool) -> bool {
        if folded && !self.foldable(path) {
            return false;
        }
        let Some(g) = self.group_mut(path) else {
            return false;
        };
        let changed = g.folded != folded;
        g.folded = folded;
        changed
    }

    /// **Whether the group at `path` may fold, or is folded**: a group in a
    /// column, beside a part that is not a folded group, which takes the
    /// room it gives up.
    pub fn foldable(&self, path: &[usize]) -> bool {
        let Some((&own, parent)) = path.split_last() else {
            return false;
        };
        if self.group(path).is_some_and(|g| g.folded) {
            return true;
        }
        match self.at(parent) {
            Some(Node::Split { row: false, parts }) => parts
                .iter()
                .enumerate()
                .any(|(i, (_, p))| i != own && !p.folded()),
            _ => false,
        }
    }

    /// **Whether the group at `path` stands in a column**, where a panel
    /// shows the fold mark: enabled while it may fold or is folded
    /// ([`foldable`](Self::foldable)), and quiet on the last open part, so
    /// a title never moves as its neighbours fold.
    pub fn in_column(&self, path: &[usize]) -> bool {
        path.split_last().is_some_and(|(_, parent)| {
            matches!(self.at(parent), Some(Node::Split { row: false, .. }))
        })
    }

    /// Whether this node is a folded group.
    pub fn folded(&self) -> bool {
        matches!(self, Node::Tabs(g) if g.folded)
    }

    /// **Puts `pane` among the tabs of the group holding `of`**, before the
    /// tab at `index` (at the end with none), and shows it.
    pub fn join(&mut self, pane: i32, of: i32, index: Option<usize>) -> Result<(), String> {
        let target = self.group_of(of).ok_or_else(|| format!("no pane {of}"))?;
        let source = self
            .group_of(pane)
            .ok_or_else(|| format!("no pane {pane}"))?;
        let mut index = index;
        if source == target
            && let (Some(g), Some(i)) = (self.group(&source), index.as_mut())
        {
            // its own tab leaves first, so a place after it moves back one
            let from = g.panes.iter().position(|&p| p == pane).unwrap_or(0);
            if from < *i {
                *i -= 1;
            }
        }
        if of == pane {
            return Ok(());
        }
        // `of` stands for its group while this one leaves
        self.remove(pane);
        let path = self.group_of(of).ok_or("the group went")?;
        let g = self.group_mut(&path).ok_or("the group went")?;
        let at = index.unwrap_or(g.panes.len()).min(g.panes.len());
        g.panes.insert(at, pane);
        g.active = at;
        g.folded = false;
        Ok(())
    }

    /// **Puts `pane` on `side` of the group holding `of`**, in a group of its
    /// own: beside it in the split that holds it when that split runs the
    /// same way, else in a split made there. The two share the group's room.
    pub fn split(&mut self, pane: i32, side: Side, of: i32) -> Result<(), String> {
        let path = self.group_of(of).ok_or_else(|| format!("no pane {of}"))?;
        self.group_of(pane)
            .ok_or_else(|| format!("no pane {pane}"))?;
        // what stands for the group once `pane` has left it
        let anchor = self
            .group(&path)
            .and_then(|g| g.panes.iter().copied().find(|&p| p != pane))
            .ok_or("a pane alone in its group cannot be split off it")?;
        self.remove(pane);
        let mut path = self.group_of(anchor).ok_or("the group went")?;
        let new = Node::Tabs(Group::of(pane));
        let parent_row = path.split_last().and_then(|(_, up)| match self.at(up) {
            Some(Node::Split { row, .. }) => Some(*row),
            _ => None,
        });
        if parent_row == Some(side.row()) {
            let i = path.pop().unwrap_or(0);
            if let Some(Node::Split { parts, .. }) = self.at_mut(&path) {
                let half = parts[i].0 * 0.5;
                parts[i].0 = half;
                let at = if side.after() { i + 1 } else { i };
                parts.insert(at, (half, new));
            }
        } else if let Some(node) = self.at_mut(&path) {
            let old = std::mem::take(node);
            let parts = if side.after() {
                vec![(0.5, old), (0.5, new)]
            } else {
                vec![(0.5, new), (0.5, old)]
            };
            *node = Node::Split {
                row: side.row(),
                parts,
            };
        }
        self.normalize();
        Ok(())
    }

    /// The shares of the split at `path`, when there is one there.
    pub fn shares(&self, path: &[usize]) -> Option<Vec<f32>> {
        match self.at(path)? {
            Node::Split { parts, .. } => Some(parts.iter().map(|(s, _)| *s).collect()),
            Node::Tabs(_) => None,
        }
    }

    /// Sets the shares of the split at `path`, normalized to sum to one.
    pub fn set_shares(&mut self, path: &[usize], shares: &[f32]) {
        if let Some(Node::Split { parts, .. }) = self.at_mut(path)
            && parts.len() == shares.len()
        {
            for ((share, _), s) in parts.iter_mut().zip(shares) {
                *share = s.max(0.0);
            }
            renormalize(parts);
        }
    }

    /// **Back to normal form**: empty groups gone, a split of one part
    /// replaced by it, a split inside one of its axis joined into it, the
    /// shares summing to one.
    pub fn normalize(&mut self) {
        self.flatten();
        self.settle_folds(false);
    }

    /// [`normalize`](Self::normalize)'s shape half: empty groups gone, a
    /// split of one part replaced by it, a split of its own axis joined in.
    fn flatten(&mut self) {
        if let Node::Split { row, parts } = self {
            for (_, part) in parts.iter_mut() {
                part.flatten();
            }
            parts.retain(|(_, p)| !matches!(p, Node::Tabs(g) if g.panes.is_empty()));
            let row = *row;
            let mut flat = Vec::with_capacity(parts.len());
            for (share, part) in parts.drain(..) {
                match part {
                    Node::Split {
                        row: inner,
                        parts: inside,
                    } if inner == row => {
                        flat.extend(inside.into_iter().map(|(s, p)| (s * share, p)));
                    }
                    other => flat.push((share, other)),
                }
            }
            *parts = flat;
            renormalize(parts);
            match parts.len() {
                0 => *self = Node::default(),
                1 => *self = parts.pop().map(|(_, p)| p).unwrap_or_default(),
                _ => {}
            }
        }
        if let Node::Tabs(g) = self {
            g.active = g.active.min(g.panes.len().saturating_sub(1));
        }
    }

    /// **A fold only where it gives room to someone**: a group outside a
    /// column (alone, or side by side with others) unfolds, and so does the
    /// last of a column whose every part is a folded group. Called from the
    /// root, after [`normalize`](Self::normalize).
    fn settle_folds(&mut self, in_column: bool) {
        match self {
            Node::Tabs(g) if !in_column => g.folded = false,
            Node::Tabs(_) => {}
            Node::Split { row, parts } => {
                let column = !*row;
                for (_, part) in parts.iter_mut() {
                    part.settle_folds(column);
                }
                if column
                    && parts.iter().all(|(_, p)| p.folded())
                    && let Some((_, Node::Tabs(g))) = parts.last_mut()
                {
                    g.folded = false;
                }
            }
        }
    }

    /// **The layout as data**: a split is `{"split": "row" | "col",
    /// "shares": [...], "parts": [...]}`, a group `{"tabs": [ids...],
    /// "active": id}`, with `"folded": true` while it is folded.
    pub fn to_json(&self) -> Value {
        match self {
            Node::Split { row, parts } => json!({
                "split": if *row { "row" } else { "col" },
                "shares": parts.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
                "parts": parts.iter().map(|(_, p)| p.to_json()).collect::<Vec<_>>(),
            }),
            Node::Tabs(g) => {
                let mut group = json!({
                    "tabs": g.panes,
                    "active": g.shown(),
                });
                // an open group says nothing of folding, as before there was any
                if g.folded {
                    group["folded"] = json!(true);
                }
                group
            }
        }
    }

    /// **A layout read back**, over the panes the dock holds: a pane it
    /// names that is not one is left out, a pane named twice keeps its first
    /// place, and a pane it does not name is added as a column of its own,
    /// so the dock shows every pane it holds whatever it was given.
    pub fn from_json(value: &Value, panes: &[i32]) -> Result<Self, String> {
        let mut node = Self::read(value, panes)?;
        let named = node.panes();
        for &p in panes {
            if !named.contains(&p) {
                node.add(p);
            }
        }
        Ok(node)
    }

    /// **A layout read back** as [`from_json`](Self::from_json) reads it,
    /// leaving out the panes it does not name: one page's layout, which need
    /// not hold every pane.
    pub fn read(value: &Value, panes: &[i32]) -> Result<Self, String> {
        let mut seen = Vec::new();
        let mut node = read(value, panes, &mut seen)?;
        node.normalize();
        Ok(node)
    }
}

/// The shares scaled to sum to one; equal ones if they summed to nothing.
fn renormalize(parts: &mut [(f32, Node)]) {
    let sum: f32 = parts.iter().map(|(s, _)| *s).sum();
    let n = parts.len() as f32;
    for (share, _) in parts.iter_mut() {
        *share = if sum > 0.0 { *share / sum } else { 1.0 / n };
    }
}

fn read(value: &Value, panes: &[i32], seen: &mut Vec<i32>) -> Result<Node, String> {
    let object = value.as_object().ok_or("a layout node is a JSON object")?;
    if let Some(tabs) = object.get("tabs") {
        let ids = tabs.as_array().ok_or("`tabs` is a list of panes")?;
        let mut group = Group {
            folded: object.get("folded").and_then(Value::as_bool) == Some(true),
            ..Group::default()
        };
        for id in ids {
            let id = id
                .as_i64()
                .and_then(|i| i32::try_from(i).ok())
                .ok_or("a tab is a pane's id")?;
            if panes.contains(&id) && !seen.contains(&id) {
                seen.push(id);
                group.panes.push(id);
            }
        }
        if let Some(active) = object.get("active").and_then(Value::as_i64) {
            group.active = group
                .panes
                .iter()
                .position(|&p| i64::from(p) == active)
                .unwrap_or(0);
        }
        return Ok(Node::Tabs(group));
    }
    let row = match object.get("split").and_then(Value::as_str) {
        Some("row") => true,
        Some("col") => false,
        _ => return Err("a layout node is `tabs` or a `split` of `row` or `col`".into()),
    };
    let parts = object
        .get("parts")
        .and_then(Value::as_array)
        .ok_or("a split has `parts`")?;
    let shares: Vec<f32> = object
        .get("shares")
        .and_then(Value::as_array)
        .map(|s| s.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
        .unwrap_or_default();
    let parts = parts
        .iter()
        .enumerate()
        .map(|(i, p)| Ok((shares.get(i).copied().unwrap_or(1.0), read(p, panes, seen)?)))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Node::Split { row, parts })
}
