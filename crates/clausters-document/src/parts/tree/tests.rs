use std::collections::BTreeMap;

use super::*;
use crate::parts::{self, RateOf};
use crate::{Lifetime, SourceRef};

fn part(source: u64, start: u64, end: u64) -> Part {
    Part {
        source: SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Temporary,
            generation: 0,
            range: Some(Range { start, end }),
        },
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: Default::default(),
        fade_out_shape: Default::default(),
        channels: None,
    }
}

/// A deterministic run of numbers.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if n == 0 { 0 } else { self.0 % n }
    }
}

/// The height a balanced tree of `node`'s parts may have, checked at every
/// node, with its counts.
fn check(node: &Node) -> (u32, u64, u64) {
    match node.children() {
        None => {
            assert_eq!(node.count(), 1);
            (1, node.frames(), 1)
        }
        Some((l, r)) => {
            let (hl, fl, cl) = check(l);
            let (hr, fr, cr) = check(r);
            assert!(hl.abs_diff(hr) <= 1, "unbalanced: {hl} against {hr}");
            assert_eq!(node.frames(), fl + fr);
            assert_eq!(node.count(), cl + cr);
            let h = hl.max(hr) + 1;
            assert_eq!(node.height, h);
            (h, fl + fr, cl + cr)
        }
    }
}

/// **A tree and the flat list read the same parts after the same edits**:
/// spans, insertions, removals and replacements at random places, with
/// fades and a take at another rate, each checked against the list's own
/// arithmetic and for balance.
#[test]
fn a_tree_and_the_flat_list_agree_after_any_edits() {
    let mut rates_of = BTreeMap::new();
    rates_of.insert(SourceId(3), 44_100.0);
    let rates = Rates {
        join: 48_000.0,
        of: &rates_of,
    };
    assert_eq!(rates_of.rate_of(SourceId(3)), Some(44_100.0));
    let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
    let mut seed = vec![part(1, 0, 10_000)];
    let mut faded = part(2, 100, 900);
    faded.fade_in = 30;
    faded.fade_out = 40;
    seed.push(faded);
    seed.push(part(3, 0, 4_410));
    let mut flat = seed.clone();
    let mut tree = Tree::from_parts(&seed, &rates).unwrap();
    let mut clip_flat: Vec<Part> = Vec::new();
    let mut clip_tree = Tree::default();
    for round in 0..600 {
        let total = parts::length(&flat, &rates).unwrap();
        assert_eq!(tree.frames(), total, "round {round}");
        let a = dice.below(total + 1);
        let b = (a + dice.below(total / 3 + 1)).min(total);
        match dice.below(4) {
            0 => {
                clip_flat = parts::span(&flat, a, b, &rates).unwrap();
                clip_tree = tree.span(a, b, &rates).unwrap();
            }
            1 => {
                flat = parts::insert(&flat, a, &clip_flat, &rates).unwrap();
                tree = tree.insert(a, &clip_tree, &rates).unwrap();
            }
            2 if total > 2_000 => {
                flat = parts::remove(&flat, a, b, &rates).unwrap();
                tree = tree.remove(a, b, &rates).unwrap();
            }
            _ => {
                flat = parts::replace(&flat, a, b, &clip_flat, &rates).unwrap();
                tree = tree.replace(a, b, &clip_tree, &rates).unwrap();
            }
        }
        assert_eq!(tree.to_vec(), flat, "round {round}");
        assert_eq!(clip_tree.to_vec(), clip_flat, "round {round}");
        if let Some(root) = tree.root() {
            check(root);
        }
        let mut want = parts::sources(&flat);
        want.sort();
        assert_eq!(tree.sources(), want.as_slice(), "round {round}");
    }
}

/// **A part read on from is one part again**: a span cut out and pasted
/// back where it was leaves the one part it was, as the flat list does.
#[test]
fn a_cut_pasted_back_is_one_part_again() {
    let tree = Tree::from_parts(&[part(1, 0, 1_000)], &Rates::SAME).unwrap();
    let cut = tree.span(200, 500, &Rates::SAME).unwrap();
    let without = tree.remove(200, 500, &Rates::SAME).unwrap();
    assert_eq!(without.count(), 2);
    let back = without.insert(200, &cut, &Rates::SAME).unwrap();
    assert_eq!(back.to_vec(), [part(1, 0, 1_000)]);
}

/// **The parts are found from a frame by descending**, each where the list
/// reads it, and nothing is found past the end.
#[test]
fn the_parts_are_read_from_any_frame() {
    let tree = Tree::from_parts(
        &[part(1, 0, 10), part(2, 0, 20), part(3, 0, 30)],
        &Rates::SAME,
    )
    .unwrap();
    let from: Vec<(u64, u64, u64)> = tree
        .iter_from(15)
        .map(|p| (p.part.source.source.0, p.start, p.end()))
        .collect();
    assert_eq!(from, [(2, 10, 30), (3, 30, 60)]);
    assert_eq!(tree.iter_from(59).count(), 1);
    assert_eq!(tree.iter_from(60).count(), 0);
    assert_eq!(Tree::default().iter_from(0).count(), 0);
}

/// **A copy pasted over and over shares its nodes**: the pattern that made
/// the flat list grow with the product of its pastes -- a span copied and
/// pasted at the end, the result copied and pasted again -- leaves a tree
/// of many parts and few nodes, each paste making only the nodes on the
/// paths to its seams, and the tree no taller than its parts allow.
#[test]
fn a_copy_pasted_over_and_over_shares_its_nodes() {
    let rates = &Rates::SAME;
    let mut take = Tree::from_parts(&[part(1, 0, 20 * 48_000)], rates).unwrap();
    let mut copied = 10 * 48_000;
    for times in [5, 9, 6, 6, 4, 3] {
        let clip = take.span(0, copied, rates).unwrap();
        for _ in 0..times {
            let before = NEXT.load(Ordering::Relaxed);
            take = take.insert(take.frames(), &clip, rates).unwrap();
            let made = NEXT.load(Ordering::Relaxed) - before;
            assert!(made < 200, "a paste made {made} nodes");
        }
        copied = take.frames();
    }
    let hours = take.frames() as f64 / 48_000.0 / 3_600.0;
    assert!(hours > 100.0, "{hours} hours");
    assert!(take.count() > 10_000, "{} parts", take.count());
    let root = take.root().unwrap();
    let (height, ..) = check(root);
    assert!(height < 40, "height {height}");
    // the distinct nodes under the root
    fn distinct(node: &Node, seen: &mut std::collections::HashSet<u64>) {
        if seen.insert(node.id())
            && let Some((l, r)) = node.children()
        {
            distinct(l, seen);
            distinct(r, seen);
        }
    }
    let mut seen = std::collections::HashSet::new();
    distinct(root, &mut seen);
    assert!(
        seen.len() < 5_000,
        "{} nodes for {} parts",
        seen.len(),
        take.count()
    );
}
