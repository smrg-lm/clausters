//! **A take's parts as a tree of shared nodes**: the list an edit is made of,
//! held so that what a copy, a paste, an undo and a redo share is shared and
//! not copied.
//!
//! A flat list of parts costs every edit the whole list: a paste writes the
//! clipboard's parts into a new list as long as the take, and a copy of a
//! span that was itself pasted many times carries all of those parts again.
//! Copied and pasted over and over, the parts grow with the product of the
//! pastes, and so does every edit after them. A [`Tree`] is the same list as
//! a balanced tree (AVL, by height) whose nodes are immutable and shared:
//! a span of it, a paste into it and a cut out of it make the few nodes on
//! the paths to the two seams and share everything else. So an edit costs the
//! height of the tree, a take pasted into itself a thousand times is a few
//! nodes, and a clipboard or a history entry holds a tree, which is one
//! reference.
//!
//! **The arithmetic is the list's** (`super`): a node knows how many frames
//! of the list it fills, a leaf is cut at a frame by [`super::span`]'s rule,
//! and two parts that read on from each other are one part again where a
//! seam brings them together -- so a tree and the flat list of the same edits
//! read the same parts, in the same order.
//!
//! **A node has an identity** ([`Node::id`]), never reused, so what a reader
//! derives from a node -- the summary a window draws it with -- can be kept
//! by that number and found again wherever the node is shared.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Rates, UNRANGED, continues, filled, merged, span};
use crate::session::Part;
use crate::{Range, SourceId};

/// The next node's identity.
static NEXT: AtomicU64 = AtomicU64::new(1);

/// **A take's parts, as a tree whose nodes are shared.** Cloning one is a
/// reference; nothing an edit makes changes a node another tree holds.
#[derive(Clone, Default)]
pub struct Tree {
    root: Option<Arc<Node>>,
}

/// **One node of a [`Tree`]**: a part, or two subtrees read one after the
/// other, with what is counted over them.
pub struct Node {
    id: u64,
    frames: u64,
    count: u64,
    height: u32,
    sources: Arc<[SourceId]>,
    kind: Kind,
}

enum Kind {
    Leaf(Part),
    Branch(Arc<Node>, Arc<Node>),
}

impl Node {
    /// **This node's identity**: no other node, live or gone, has it.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// How many frames of the list it fills.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// How many parts it holds.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// The two subtrees it reads in order, for a branch.
    pub fn children(&self) -> Option<(&Node, &Node)> {
        match &self.kind {
            Kind::Branch(l, r) => Some((l, r)),
            Kind::Leaf(_) => None,
        }
    }

    /// The part, for a leaf.
    pub fn part(&self) -> Option<&Part> {
        match &self.kind {
            Kind::Leaf(part) => Some(part),
            Kind::Branch(..) => None,
        }
    }

    /// Every take it reads, once each, in ascending order.
    pub fn sources(&self) -> &[SourceId] {
        &self.sources
    }
}

/// A leaf over `part`, filling `frames` of the list.
fn leaf(part: Part, frames: u64) -> Arc<Node> {
    let sources: Arc<[SourceId]> = Arc::from([part.source.source]);
    Arc::new(Node {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        frames,
        count: 1,
        height: 1,
        sources,
        kind: Kind::Leaf(part),
    })
}

/// The sorted union of two sorted sets, sharing either when it holds the
/// other.
fn union(a: &Arc<[SourceId]>, b: &Arc<[SourceId]>) -> Arc<[SourceId]> {
    if Arc::ptr_eq(a, b) {
        return Arc::clone(a);
    }
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x == y => {
                out.push(*x);
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x < y => {
                out.push(*x);
                i += 1;
            }
            (Some(_), Some(y)) => {
                out.push(*y);
                j += 1;
            }
            (Some(x), None) => {
                out.push(*x);
                i += 1;
            }
            (None, Some(y)) => {
                out.push(*y);
                j += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    if out.len() == a.len() {
        Arc::clone(a)
    } else if out.len() == b.len() {
        Arc::clone(b)
    } else {
        Arc::from(out)
    }
}

/// A branch reading `l` then `r`, whose heights are within one of each other.
fn branch(l: Arc<Node>, r: Arc<Node>) -> Arc<Node> {
    Arc::new(Node {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        frames: l.frames + r.frames,
        count: l.count + r.count,
        height: l.height.max(r.height) + 1,
        sources: union(&l.sources, &r.sources),
        kind: Kind::Branch(l, r),
    })
}

fn split_branch(node: &Arc<Node>) -> (Arc<Node>, Arc<Node>) {
    match &node.kind {
        Kind::Branch(l, r) => (Arc::clone(l), Arc::clone(r)),
        Kind::Leaf(_) => unreachable!("a node taller than its sibling by two is a branch"),
    }
}

/// **`l` then `r` as one node**, their heights within two of each other: a
/// rotation where they are two apart, as an AVL tree does. A branch here has
/// no key, so a rotation is only a regrouping.
fn balance(l: Arc<Node>, r: Arc<Node>) -> Arc<Node> {
    if l.height > r.height + 1 {
        let (a, b) = split_branch(&l);
        if a.height >= b.height {
            branch(a, branch(b, r))
        } else {
            let (b1, b2) = split_branch(&b);
            branch(branch(a, b1), branch(b2, r))
        }
    } else if r.height > l.height + 1 {
        let (a, b) = split_branch(&r);
        if b.height >= a.height {
            branch(branch(l, a), b)
        } else {
            let (a1, a2) = split_branch(&a);
            branch(branch(l, a1), branch(a2, b))
        }
    } else {
        branch(l, r)
    }
}

/// **`l` then `r`**, balanced: the work is the difference of their heights.
fn join(l: Option<Arc<Node>>, r: Option<Arc<Node>>) -> Option<Arc<Node>> {
    let (l, r) = match (l, r) {
        (None, r) => return r,
        (l, None) => return l,
        (Some(l), Some(r)) => (l, r),
    };
    Some(if l.height > r.height + 1 {
        let (a, b) = split_branch(&l);
        let t = join(Some(b), Some(r)).expect("two trees join into one");
        balance(a, t)
    } else if r.height > l.height + 1 {
        let (a, b) = split_branch(&r);
        let t = join(Some(l), Some(a)).expect("two trees join into one");
        balance(t, b)
    } else {
        branch(l, r)
    })
}

/// What is before a cut and what is after it, either of which may be nothing.
type Halves = (Option<Arc<Node>>, Option<Arc<Node>>);

/// **`node` cut at frame `at` of the list**: what is before it and what is
/// after it, a part under `at` cut in two by [`span`]'s rule.
fn split(node: &Arc<Node>, at: u64, rates: &Rates) -> Result<Halves, &'static str> {
    if at == 0 {
        return Ok((None, Some(Arc::clone(node))));
    }
    if at >= node.frames {
        return Ok((Some(Arc::clone(node)), None));
    }
    match &node.kind {
        Kind::Leaf(part) => {
            let one = std::slice::from_ref(part);
            let side = |from: u64, to: u64| -> Result<Option<Arc<Node>>, &'static str> {
                Ok(span(one, from, to, rates)?
                    .into_iter()
                    .next()
                    .and_then(|p| ranged_leaf(p, rates)))
            };
            Ok((side(0, at)?, side(at, node.frames)?))
        }
        Kind::Branch(l, r) => {
            if at < l.frames {
                let (a, b) = split(l, at, rates)?;
                Ok((a, join(b, Some(Arc::clone(r)))))
            } else if at == l.frames {
                Ok((Some(Arc::clone(l)), Some(Arc::clone(r))))
            } else {
                let (a, b) = split(r, at - l.frames, rates)?;
                Ok((join(Some(Arc::clone(l)), a), b))
            }
        }
    }
}

/// A leaf over `part` counted at `rates`, or none for a part of no frames.
fn ranged_leaf(part: Part, rates: &Rates) -> Option<Arc<Node>> {
    let range = part.source.range?;
    if range.is_empty() {
        return None;
    }
    let frames = filled(&part, range, rates);
    Some(leaf(part, frames))
}

/// A balanced tree over leaves already made.
fn build(leaves: &[Arc<Node>]) -> Option<Arc<Node>> {
    match leaves.len() {
        0 => None,
        1 => Some(Arc::clone(&leaves[0])),
        n => {
            let (l, r) = leaves.split_at(n / 2);
            Some(branch(build(l)?, build(r)?))
        }
    }
}

fn first(mut node: &Node) -> &Part {
    loop {
        match &node.kind {
            Kind::Leaf(part) => return part,
            Kind::Branch(l, _) => node = l,
        }
    }
}

fn last(mut node: &Node) -> (&Part, u64) {
    loop {
        match &node.kind {
            Kind::Leaf(part) => return (part, node.frames),
            Kind::Branch(_, r) => node = r,
        }
    }
}

impl Tree {
    /// **The tree of `parts`**, counted at `rates`: the list as
    /// [`super::replace`] would leave it, two parts that read on from each
    /// other made one. A part that does not say its frames is refused.
    pub fn from_parts(parts: &[Part], rates: &Rates) -> Result<Tree, &'static str> {
        if parts.iter().any(|p| p.source.range.is_none()) {
            return Err(UNRANGED);
        }
        let leaves: Vec<Arc<Node>> = merged(parts.to_vec())
            .into_iter()
            .filter_map(|p| ranged_leaf(p, rates))
            .collect();
        Ok(Tree {
            root: build(&leaves),
        })
    }

    /// **The tree of parts whose frames are said**: each part with how many
    /// frames of the list it fills, as a reader that never cuts one is told
    /// them. A part of no frames is left out; nothing is merged.
    pub fn from_counted(parts: impl IntoIterator<Item = (Part, u64)>) -> Tree {
        let leaves: Vec<Arc<Node>> = parts
            .into_iter()
            .filter(|(_, frames)| *frames > 0)
            .map(|(part, frames)| leaf(part, frames))
            .collect();
        Tree {
            root: build(&leaves),
        }
    }

    /// The root, for a reader that walks the nodes.
    pub fn root(&self) -> Option<&Node> {
        self.root.as_deref()
    }

    /// How many frames the list is.
    pub fn frames(&self) -> u64 {
        self.root.as_ref().map_or(0, |n| n.frames)
    }

    /// How many parts it holds.
    pub fn count(&self) -> u64 {
        self.root.as_ref().map_or(0, |n| n.count)
    }

    /// Whether it holds no part.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// **Every take it reads**, once each, in ascending order: what a
    /// history or a clipboard holding it keeps named.
    pub fn sources(&self) -> &[SourceId] {
        self.root.as_ref().map_or(&[], |n| &n.sources)
    }

    /// Whether the two are the same tree, not merely equal ones.
    pub fn same(&self, other: &Tree) -> bool {
        match (&self.root, &other.root) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    /// **The parts in order**, each with where it starts in the list and how
    /// many frames it fills.
    pub fn iter(&self) -> Iter<'_> {
        self.iter_from(0)
    }

    /// **The parts from the one under frame `frame` on**, as [`Self::iter`]
    /// reads them: found by descending, so the cost is the tree's height and
    /// not the parts before it. Nothing past the end.
    pub fn iter_from(&self, frame: u64) -> Iter<'_> {
        let mut iter = Iter { stack: Vec::new() };
        let Some(mut node) = self.root.as_deref() else {
            return iter;
        };
        if frame >= node.frames {
            return iter;
        }
        let mut offset = 0;
        loop {
            match &node.kind {
                Kind::Leaf(_) => {
                    iter.stack.push((node, offset));
                    break;
                }
                Kind::Branch(l, r) => {
                    if frame < offset + l.frames {
                        iter.stack.push((r, offset + l.frames));
                        node = l;
                    } else {
                        offset += l.frames;
                        node = r;
                    }
                }
            }
        }
        iter
    }

    /// The parts as a flat list.
    pub fn to_vec(&self) -> Vec<Part> {
        self.iter().map(|placed| placed.part.clone()).collect()
    }

    /// **Frames `from`..`to` of the list**, as [`super::span`] cuts them,
    /// sharing every node the span holds whole.
    pub fn span(&self, from: u64, to: u64, rates: &Rates) -> Result<Tree, &'static str> {
        let Some(root) = &self.root else {
            return Ok(Tree::default());
        };
        let to = to.min(root.frames);
        if from >= to {
            return Ok(Tree::default());
        }
        // Cut at the end first, so both cuts of a part count from where it
        // starts, as the flat list cuts it.
        let (within, _) = split(root, to, rates)?;
        let Some(within) = within else {
            return Ok(Tree::default());
        };
        let (_, within) = split(&within, from, rates)?;
        Ok(Tree { root: within })
    }

    /// **`self` then `other`**, two parts that meet at the seam and read on
    /// from each other made one, as [`super::replace`] leaves them.
    pub fn concat(&self, other: &Tree, rates: &Rates) -> Result<Tree, &'static str> {
        let (Some(a), Some(b)) = (&self.root, &other.root) else {
            return Ok(Tree {
                root: self.root.clone().or_else(|| other.root.clone()),
            });
        };
        let (tail, tail_frames) = last(a);
        let head = first(b);
        if !continues(tail, head) {
            return Ok(Tree {
                root: join(Some(Arc::clone(a)), Some(Arc::clone(b))),
            });
        }
        let (Some(x), Some(y)) = (tail.source.range, head.source.range) else {
            unreachable!("continues() reads both ranges");
        };
        let mut one = tail.clone();
        one.source.range = Some(Range {
            start: x.start,
            end: y.end,
        });
        one.fade_out = head.fade_out;
        one.fade_out_shape = head.fade_out_shape;
        let (before, _) = split(a, a.frames - tail_frames, rates)?;
        let after = without_first(b);
        Ok(Tree {
            root: join(join(before, ranged_leaf(one, rates)), after),
        })
    }

    /// **The list with frames `from`..`to` replaced by `with`** -- a paste
    /// over a selection, a take drawn over a span, a deletion (`with` empty).
    /// A position past the end is the end.
    pub fn replace(
        &self,
        from: u64,
        to: u64,
        with: &Tree,
        rates: &Rates,
    ) -> Result<Tree, &'static str> {
        let total = self.frames();
        let from = from.min(total);
        let to = to.clamp(from, total);
        // Both cuts from the list as it is, as the flat list makes them: a
        // part under both is cut at each counting from where it starts.
        let (before, after) = match &self.root {
            None => (None, None),
            Some(root) => (split(root, from, rates)?.0, split(root, to, rates)?.1),
        };
        Tree { root: before }
            .concat(with, rates)?
            .concat(&Tree { root: after }, rates)
    }

    /// **The list with frames `from`..`to` taken out.**
    pub fn remove(&self, from: u64, to: u64, rates: &Rates) -> Result<Tree, &'static str> {
        self.replace(from, to, &Tree::default(), rates)
    }

    /// **The list with `inserted` put in at frame `at`.**
    pub fn insert(&self, at: u64, inserted: &Tree, rates: &Rates) -> Result<Tree, &'static str> {
        self.replace(at, at, inserted, rates)
    }
}

/// `node` without its first part.
fn without_first(node: &Arc<Node>) -> Option<Arc<Node>> {
    match &node.kind {
        Kind::Leaf(_) => None,
        Kind::Branch(l, r) => join(without_first(l), Some(Arc::clone(r))),
    }
}

/// **One part where the list reads it**: its first frame in the list, how
/// many frames of the list it fills, and the part.
#[derive(Clone, Copy, Debug)]
pub struct Placed<'a> {
    pub start: u64,
    pub frames: u64,
    pub part: &'a Part,
}

impl Placed<'_> {
    /// One past its last frame in the list.
    pub fn end(&self) -> u64 {
        self.start + self.frames
    }
}

/// The parts of a [`Tree`] in order ([`Tree::iter`], [`Tree::iter_from`]).
pub struct Iter<'a> {
    stack: Vec<(&'a Node, u64)>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Placed<'a>;

    fn next(&mut self) -> Option<Placed<'a>> {
        let (mut node, offset) = self.stack.pop()?;
        loop {
            match &node.kind {
                Kind::Leaf(part) => {
                    return Some(Placed {
                        start: offset,
                        frames: node.frames,
                        part,
                    });
                }
                Kind::Branch(l, r) => {
                    self.stack.push((r, offset + l.frames));
                    node = l;
                }
            }
        }
    }
}

impl PartialEq for Tree {
    /// The same parts filling the same frames, in the same order.
    fn eq(&self, other: &Tree) -> bool {
        self.same(other)
            || (self.frames() == other.frames()
                && self.count() == other.count()
                && self
                    .iter()
                    .zip(other.iter())
                    .all(|(a, b)| a.frames == b.frames && a.part == b.part))
    }
}

impl std::fmt::Debug for Tree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(|p| p.part)).finish()
    }
}

impl serde::Serialize for Tree {
    /// As the flat list of its parts, which is the list's own form.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(|p| p.part))
    }
}

#[cfg(test)]
mod tests;
