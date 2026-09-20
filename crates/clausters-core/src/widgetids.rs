//! The widget-id table: an id names **what is drawn**, not the order it was
//! drawn in.
//!
//! A GUI host's widget namespace is allocated by its clients, and until now it
//! was allocated the way node ids are: a [`crate::registry::Registry`]
//! hands one out per widget as a tree is built, and a redraw returns the whole
//! subtree and asks for it again. That makes an id a **lease** rather than an
//! identity, and the consequences are not subtle:
//!
//! - anything in flight across a redraw lands on the wrong widget -- an
//!   edit-back the owner has not answered yet, a pending gesture, a correction
//!   travelling the other way;
//! - screen state cannot survive a redraw, because the widget it belonged to no
//!   longer exists under that number;
//! - and none of it is reproducible, since which id a widget gets depends on the
//!   order the tree happened to be built in.
//!
//! So this table adds the other door. A **keyed** id is asked for by naming what
//! it draws -- the structure's identity in the history, the role the widget plays
//! in its view, and which one it is within that role -- and the same name gets
//! the same number for as long as it keeps being drawn. Both doors share one
//! occupancy map, which is what makes them impossible to collide: a keyed id and
//! an anonymous one are the same resource taken two ways, not two spaces that
//! have to be kept apart by arithmetic.
//!
//! # The draw cycle, and who is drawing
//!
//! Keyed ids are held across draws, so something has to say when a widget has
//! stopped being drawn. That is [`WidgetIds::begin`] and [`WidgetIds::retire`],
//! and both name an **owner** -- because one table serves a whole host, and a
//! host carries more than one drawer. Two editors opened on the ambient host
//! are two owners of one namespace, and a cycle that did not say whose it was
//! would let either one retire the other's widgets simply by redrawing.
//!
//! ```
//! use clausters_core::widgetids::WidgetIds;
//!
//! let mut ids = WidgetIds::new(1000, 64);
//! let who = ids.owner();
//!
//! ids.begin(who);
//! let clip = ids.id_for(who, 7, "clip", "3").unwrap();
//! assert!(ids.retire(who).is_empty());        // it was drawn, so it stays
//!
//! ids.begin(who);                             // a redraw that does not draw it
//! assert_eq!(ids.retire(who), vec![clip]);    // ...and the id comes back
//! assert_eq!(ids.in_use(), 0);                //    to a space that is now free
//! ```
//!
//! A name asked for again after that is a **new** widget and gets a fresh id:
//! what the table promises is that a widget still being drawn keeps its number,
//! not that a number is reserved for a name that stopped.
//!
//! A draw that never calls `begin` is an anonymous draw, and `retire` then has
//! nothing to say: a key is only stale relative to a draw that was declared.
//!
//! **Nothing here hashes an id out of a name.** A 31-bit space and a thousand
//! live widgets is a collision every few thousand sessions, and a collision is
//! two widgets answering to one number -- silent, and indistinguishable from the
//! bug this table exists to remove. A map costs a lookup and cannot do that.

use std::collections::{HashMap, HashSet};

use crate::registry::Registry;

/// The separator inside a composed key. A control character, so a role or a
/// caller's own key can hold anything printable without ambiguity.
const SEP: char = '\u{1}';

/// One client's widget-id space: the occupancy map, plus the names the keyed
/// ids in it answer to.
///
/// Both doors take from the same map -- [`alloc`](Self::alloc) for a widget
/// nothing names (a hand-built tree, a decoration) and
/// [`id_for`](Self::id_for) for one that draws something -- so an id is unique
/// across everything this client has named, whichever way it was asked for.
pub struct WidgetIds {
    ids: Registry,
    /// The composed key -> the id it was given, and who drew it.
    by_key: HashMap<String, Named>,
    /// Per owner, the keys asked for since that owner's last
    /// [`begin`](Self::begin). An owner absent here is not drawing.
    drawn: HashMap<i64, HashSet<String>>,
    /// The ids that answer to a name, so an anonymous [`free`](Self::free)
    /// can refuse to take one back. Kept beside `by_key` rather than derived
    /// from it because `free` is called once per widget of a redefined tree.
    named_ids: HashSet<i64>,
    /// The next owner [`owner`](Self::owner) hands out.
    next_owner: i64,
}

/// One named id: the number, and the drawer whose cycle retires it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Named {
    id: i64,
    owner: i64,
}

impl WidgetIds {
    /// A table over `[base, base + capacity)`.
    pub fn new(base: i64, capacity: usize) -> Self {
        Self {
            ids: Registry::new(base, capacity),
            by_key: HashMap::new(),
            drawn: HashMap::new(),
            named_ids: HashSet::new(),
            next_owner: 1,
        }
    }

    /// A table whose space never runs out -- the offline counterpart, matching
    /// [`Registry::unbounded`](crate::registry::Registry::unbounded).
    pub fn unbounded(base: i64) -> Self {
        Self {
            ids: Registry::unbounded(base),
            by_key: HashMap::new(),
            drawn: HashMap::new(),
            named_ids: HashSet::new(),
            next_owner: 1,
        }
    }

    /// The composed name of one widget: what it draws, the role it plays and
    /// which one it is.
    ///
    /// Composed **here** rather than by each caller, so two clients naming the
    /// same widget cannot spell it differently -- which is the whole reason the
    /// table is in the shared core rather than written once per language.
    pub fn key(structure: i64, role: &str, key: &str) -> String {
        let mut composed = String::with_capacity(role.len() + key.len() + 24);
        composed.push_str(&structure.to_string());
        composed.push(SEP);
        composed.push_str(role);
        composed.push(SEP);
        composed.push_str(key);
        composed
    }

    /// A fresh drawer: the owner a [`begin`](Self::begin)/[`retire`](Self::retire)
    /// cycle names, and the one whose keys that cycle may take back.
    ///
    /// One table serves a whole host, so a drawer has to be a value the table
    /// hands out rather than something a caller invents: two clients inventing
    /// their own would eventually pick the same one, and each would then retire
    /// the other's widgets by redrawing.
    pub fn owner(&mut self) -> i64 {
        let owner = self.next_owner;
        self.next_owner += 1;
        owner
    }

    /// An id nothing names: the lease, for a widget no structure is behind.
    /// `None` when the space is full.
    pub fn alloc(&mut self) -> Option<i64> {
        self.ids.alloc(1)
    }

    /// The id that draws this name, minted on first ask and the same one after
    /// that. `None` only when the space is full.
    ///
    /// Asking marks the widget as drawn in the current cycle, which is what
    /// keeps [`retire`](Self::retire) from taking it back.
    pub fn id_for(&mut self, owner: i64, structure: i64, role: &str, key: &str) -> Option<i64> {
        let composed = Self::key(structure, role, key);
        if let Some(named) = self.by_key.get_mut(&composed) {
            // **The owner follows the drawer**, so a widget picked up by a
            // second view is retired by the one that is actually drawing it
            // rather than by the one that drew it first.
            named.owner = owner;
            let id = named.id;
            if let Some(drawn) = self.drawn.get_mut(&owner) {
                drawn.insert(composed);
            }
            return Some(id);
        }
        let id = self.ids.alloc(1)?;
        if let Some(drawn) = self.drawn.get_mut(&owner) {
            drawn.insert(composed.clone());
        }
        self.by_key.insert(composed, Named { id, owner });
        self.named_ids.insert(id);
        Some(id)
    }

    /// The id that draws this name **if it already has one** -- no minting, and
    /// no effect on the draw cycle.
    ///
    /// The inverse a view needs in order to ask "what id is drawing this?"
    /// without becoming the thing that decides.
    pub fn id_of(&self, structure: i64, role: &str, key: &str) -> Option<i64> {
        self.by_key
            .get(&Self::key(structure, role, key))
            .map(|named| named.id)
    }

    /// Give one keyed id back by name, whether or not a draw is running.
    /// Answers the id released, if there was one.
    pub fn forget(&mut self, structure: i64, role: &str, key: &str) -> Option<i64> {
        let composed = Self::key(structure, role, key);
        let named = self.by_key.remove(&composed)?;
        for drawn in self.drawn.values_mut() {
            drawn.remove(&composed);
        }
        self.named_ids.remove(&named.id);
        let _ = self.ids.release(named.id, 1);
        Some(named.id)
    }

    /// Return an **anonymous** id to the space. Ids this table never handed
    /// out, and ids outside its window, are ignored -- so freeing is always
    /// safe.
    ///
    /// An id that answers to a name is ignored too, and that is the load-bearing
    /// half: a host frees a redefined subtree widget by widget, and a keyed id
    /// is still held by its name at that moment. Letting an anonymous free take
    /// one back would hand the same number out twice -- the exact failure keyed
    /// ids exist to remove. A named id leaves only through
    /// [`retire`](Self::retire) or [`forget`](Self::forget).
    pub fn free(&mut self, id: i64) {
        if self.named_ids.contains(&id) {
            return;
        }
        let _ = self.ids.release(id, 1);
    }

    /// Start `owner`'s draw: from here until its [`retire`](Self::retire),
    /// every keyed id that owner asks for counts as still drawn.
    ///
    /// Another owner's cycle is untouched, which is the whole point of naming
    /// one: two drawers on one host redraw independently.
    pub fn begin(&mut self, owner: i64) {
        self.drawn.insert(owner, HashSet::new());
    }

    /// How many keyed ids `owner`'s current draw would take back -- nothing when
    /// that owner is not drawing.
    ///
    /// It exists so a caller that has to size a buffer before
    /// [`retire`](Self::retire) can do it without the destructive call: a
    /// binding that under-sized its buffer must be able to refuse *before* the
    /// ids are gone rather than after.
    pub fn stale(&self, owner: i64) -> usize {
        let Some(drawn) = self.drawn.get(&owner) else {
            return 0;
        };
        self.by_key
            .iter()
            .filter(|(key, named)| named.owner == owner && !drawn.contains(*key))
            .count()
    }

    /// End `owner`'s draw and take back every keyed id of that owner's that was
    /// **not** asked for in it, answering the ids released, ascending.
    ///
    /// Ascending rather than in the order they were found, because a
    /// `HashMap`'s order is not one: what a caller does with these is free a
    /// widget per id, and two clients doing that in different orders is a
    /// difference nobody decided.
    pub fn retire(&mut self, owner: i64) -> Vec<i64> {
        let Some(drawn) = self.drawn.remove(&owner) else {
            return Vec::new();
        };
        let stale: Vec<String> = self
            .by_key
            .iter()
            .filter(|(key, named)| named.owner == owner && !drawn.contains(*key))
            .map(|(key, _)| key.clone())
            .collect();
        let mut released = Vec::with_capacity(stale.len());
        for key in stale {
            if let Some(named) = self.by_key.remove(&key) {
                self.named_ids.remove(&named.id);
                let _ = self.ids.release(named.id, 1);
                released.push(named.id);
            }
        }
        released.sort_unstable();
        released
    }

    /// How many ids are held, keyed and anonymous together.
    pub fn in_use(&self) -> usize {
        self.ids.in_use()
    }

    /// How many of them answer to a name.
    pub fn named(&self) -> usize {
        self.by_key.len()
    }

    /// Whether `id` falls in this table's space -- the filter for a foreign id.
    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(id)
    }

    /// Drop every name and every id: the table as it was made.
    pub fn clear(&mut self) {
        self.ids.clear();
        self.by_key.clear();
        self.named_ids.clear();
        self.drawn.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table and its one drawer, which is what most of these need.
    fn table(base: i64, capacity: usize) -> (WidgetIds, i64) {
        let mut ids = WidgetIds::new(base, capacity);
        let who = ids.owner();
        (ids, who)
    }

    #[test]
    fn a_name_keeps_its_id_across_draws() {
        let (mut ids, who) = table(1000, 16);
        ids.begin(who);
        let first = ids.id_for(who, 1, "curve", "0").unwrap();
        ids.retire(who);
        ids.begin(who);
        assert_eq!(ids.id_for(who, 1, "curve", "0"), Some(first));
        ids.retire(who);
    }

    #[test]
    fn the_two_doors_share_one_space() {
        let (mut ids, who) = table(0, 3);
        let anon = ids.alloc().unwrap();
        let keyed = ids.id_for(who, 1, "r", "k").unwrap();
        assert_ne!(anon, keyed);
        assert_eq!(ids.in_use(), 2);
    }

    #[test]
    fn different_names_are_different_ids() {
        let (mut ids, who) = table(1000, 16);
        let a = ids.id_for(who, 1, "clip", "1").unwrap();
        let b = ids.id_for(who, 1, "clip", "2").unwrap();
        let c = ids.id_for(who, 2, "clip", "1").unwrap();
        let d = ids.id_for(who, 1, "lane", "1").unwrap();
        let mut all = vec![a, b, c, d];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 4, "structure, role and key all name");
    }

    #[test]
    fn a_widget_that_stopped_being_drawn_gives_its_id_back() {
        let (mut ids, who) = table(1000, 16);
        ids.begin(who);
        let kept = ids.id_for(who, 1, "clip", "1").unwrap();
        let gone = ids.id_for(who, 1, "clip", "2").unwrap();
        assert!(ids.retire(who).is_empty(), "both were drawn");

        ids.begin(who);
        assert_eq!(ids.id_for(who, 1, "clip", "1"), Some(kept));
        assert_eq!(ids.retire(who), vec![gone]);
        assert_eq!(ids.named(), 1);
        assert_eq!(ids.in_use(), 1);
    }

    #[test]
    fn a_retired_id_is_allocatable_again() {
        let (mut ids, who) = table(0, 1);
        ids.begin(who);
        let only = ids.id_for(who, 1, "r", "a").unwrap();
        ids.begin(who);
        assert_eq!(ids.retire(who), vec![only]);
        ids.begin(who);
        assert_eq!(ids.id_for(who, 1, "r", "b"), Some(only));
    }

    #[test]
    fn id_of_looks_up_without_minting() {
        let (mut ids, who) = table(1000, 16);
        assert_eq!(ids.id_of(1, "r", "a"), None);
        assert_eq!(ids.in_use(), 0, "a lookup is not an ask");
        let id = ids.id_for(who, 1, "r", "a").unwrap();
        assert_eq!(ids.id_of(1, "r", "a"), Some(id));
    }

    #[test]
    fn a_draw_that_was_never_declared_retires_nothing() {
        let (mut ids, who) = table(1000, 16);
        ids.id_for(who, 1, "r", "a").unwrap();
        assert!(
            ids.retire(who).is_empty(),
            "a key is stale only against a draw"
        );
        assert_eq!(ids.named(), 1);
    }

    #[test]
    fn one_drawer_never_retires_anothers_widgets() {
        // The reason a cycle names an owner at all: two editors opened on one
        // ambient host share this table, and either redrawing would otherwise
        // take back everything the other had.
        let mut ids = WidgetIds::new(1000, 16);
        let (left, right) = (ids.owner(), ids.owner());
        ids.begin(left);
        let mine = ids.id_for(left, 1, "curve", "0").unwrap();
        ids.retire(left);

        ids.begin(right);
        let theirs = ids.id_for(right, 2, "roll", "0").unwrap();
        assert!(
            ids.retire(right).is_empty(),
            "the other drawer is untouched"
        );

        ids.begin(left);
        assert_eq!(ids.id_for(left, 1, "curve", "0"), Some(mine));
        assert!(ids.retire(left).is_empty());
        assert_eq!(ids.id_of(2, "roll", "0"), Some(theirs), "and still theirs");
    }

    #[test]
    fn a_widget_taken_over_is_retired_by_whoever_draws_it() {
        let mut ids = WidgetIds::new(1000, 16);
        let (left, right) = (ids.owner(), ids.owner());
        ids.begin(left);
        let id = ids.id_for(left, 1, "curve", "0").unwrap();
        ids.retire(left);

        ids.begin(right);
        assert_eq!(ids.id_for(right, 1, "curve", "0"), Some(id), "same widget");
        ids.retire(right);

        ids.begin(left);
        assert!(
            ids.retire(left).is_empty(),
            "no longer this drawer's to take"
        );
        ids.begin(right);
        assert_eq!(ids.retire(right), vec![id]);
    }

    #[test]
    fn an_anonymous_free_cannot_take_back_a_named_id() {
        // A host frees a redefined subtree widget by widget, and a keyed id is
        // still held by its name at that moment.
        let (mut ids, who) = table(0, 2);
        let named = ids.id_for(who, 1, "clip", "1").unwrap();
        ids.free(named);
        assert_eq!(ids.in_use(), 1, "the name still holds it");
        assert_ne!(ids.alloc(), Some(named), "so it is not handed out twice");
        assert_eq!(ids.forget(1, "clip", "1"), Some(named));
        assert_eq!(ids.in_use(), 1, "and now only the anonymous one is held");
    }

    #[test]
    fn exhaustion_is_reported_and_never_wraps() {
        let (mut ids, who) = table(0, 1);
        assert!(ids.id_for(who, 1, "r", "a").is_some());
        assert_eq!(ids.id_for(who, 1, "r", "b"), None);
        assert_eq!(ids.alloc(), None);
    }

    #[test]
    fn forget_takes_one_name_back_outside_a_draw() {
        let (mut ids, who) = table(1000, 16);
        let id = ids.id_for(who, 1, "r", "a").unwrap();
        assert_eq!(ids.forget(1, "r", "a"), Some(id));
        assert_eq!(ids.forget(1, "r", "a"), None);
        assert_eq!(ids.in_use(), 0);
    }

    #[test]
    fn stale_counts_what_retire_would_take() {
        let (mut ids, who) = table(1000, 16);
        ids.begin(who);
        ids.id_for(who, 1, "r", "a").unwrap();
        ids.id_for(who, 1, "r", "b").unwrap();
        ids.retire(who);
        ids.begin(who);
        ids.id_for(who, 1, "r", "a").unwrap();
        assert_eq!(ids.stale(who), 1);
        assert_eq!(ids.retire(who).len(), 1);
        assert_eq!(ids.stale(who), 0, "and nothing once the draw is over");
    }

    #[test]
    fn the_key_is_composed_here_so_two_clients_cannot_differ() {
        assert_eq!(WidgetIds::key(7, "clip", "3"), "7\u{1}clip\u{1}3");
        // ...and a separator that cannot be typed into a role keeps the three
        // parts apart: these are different widgets, not one.
        let (mut ids, who) = table(1000, 16);
        let a = ids.id_for(who, 1, "a", "bc").unwrap();
        let b = ids.id_for(who, 1, "ab", "c").unwrap();
        assert_ne!(a, b);
    }
}
