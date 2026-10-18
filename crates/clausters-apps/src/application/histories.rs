//! Editing orders and object membership, independent of an editor vocabulary.

use std::collections::{BTreeSet, HashMap};
use std::ops::{Deref, DerefMut};

use clausters_document::SourceId;
use clausters_document::history::{History, StructureId};

/// An undo order within an application service. Zero is the shared context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u64);

/// An object observed by a member. Its number is independent of server ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Object {
    /// The member observing it.
    pub member: u32,
    /// The member's stable object number.
    pub id: i64,
}

#[derive(Debug, Default)]
struct Order {
    history: History,
    keys: HashMap<String, StructureId>,
}

#[derive(Clone, Copy, Debug)]
struct Binding {
    scope: ScopeId,
    structure: StructureId,
}

/// Named shared orders and independent object orders over the existing engine.
///
/// Selecting an order changes neither another order's cursor nor its storage.
/// Source retention is read across all orders, including inactive ones.
#[derive(Debug)]
pub struct Histories {
    selected: ScopeId,
    next: u64,
    orders: HashMap<ScopeId, Order>,
    objects: HashMap<Object, Binding>,
    released: BTreeSet<SourceId>,
}

impl Default for Histories {
    fn default() -> Self {
        Self::new()
    }
}

impl Histories {
    /// A service whose selected order is the shared context.
    pub fn new() -> Self {
        Self {
            orders: [(ScopeId(0), Order::default())].into(),
            selected: ScopeId(0),
            next: 0,
            objects: HashMap::new(),
            released: BTreeSet::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn orders_len(&self) -> usize {
        self.orders.len()
    }

    /// The selected order.
    pub fn selected(&self) -> ScopeId {
        self.selected
    }

    /// Select an existing order, or the shared context when it was removed.
    pub fn select(&mut self, scope: ScopeId) {
        self.selected = if self.orders.contains_key(&scope) {
            scope
        } else {
            ScopeId(0)
        };
    }

    /// Join the context under a key; a second observer shares its structure.
    pub fn join(&mut self, object: Object, key: &str, domain: &str) -> StructureId {
        if let Some(binding) = self.objects.get(&object) {
            return binding.structure;
        }
        let order = self.orders.entry(ScopeId(0)).or_default();
        let structure = *order
            .keys
            .entry(key.into())
            .or_insert_with(|| order.history.register(domain));
        self.objects.insert(
            object,
            Binding {
                scope: ScopeId(0),
                structure,
            },
        );
        structure
    }

    /// Select an object's order, creating an independent one if it is new.
    pub fn focus(&mut self, object: Object, domain: &str) -> StructureId {
        let binding = *self.objects.entry(object).or_insert_with(|| {
            self.next += 1;
            let scope = ScopeId(self.next);
            let mut order = Order::default();
            let structure = order.history.register(domain);
            self.orders.insert(scope, order);
            Binding { scope, structure }
        });
        self.selected = binding.scope;
        binding.structure
    }

    /// Forget an observer; the last observer removes its structure and order.
    pub fn close(&mut self, object: Object) {
        let Some(binding) = self.objects.remove(&object) else {
            return;
        };
        if self
            .objects
            .values()
            .any(|b| b.structure == binding.structure)
        {
            return;
        }
        let order = self.orders.get_mut(&binding.scope).expect("bound order");
        order.history.forget(binding.structure);
        order.keys.retain(|_, id| *id != binding.structure);
        self.released.extend(order.history.released_sources());
        if binding.scope != ScopeId(0) {
            self.released.extend(order.history.held_sources());
            self.orders.remove(&binding.scope);
            if self.selected == binding.scope {
                self.selected = ScopeId(0);
            }
        }
    }

    /// A structure registered under a shared-context key.
    pub fn named(&self, key: &str) -> Option<StructureId> {
        self.orders.get(&ScopeId(0))?.keys.get(key).copied()
    }

    /// Every source retained by any order, deduplicated.
    pub fn held_sources(&self) -> Vec<SourceId> {
        self.orders
            .values()
            .flat_map(|o| o.history.held_sources())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// All structures observed by a member, including inactive objects.
    pub fn structures_for(&self, member: u32) -> Vec<StructureId> {
        self.objects
            .iter()
            .filter(|(object, _)| object.member == member)
            .map(|(_, binding)| binding.structure)
            .collect()
    }

    /// Apply an endpoint-aware byte budget independently to each order.
    pub fn trim_orders(&mut self, bytes: u64, held_bytes: &dyn Fn(&History) -> u64) {
        for order in self.orders.values_mut() {
            order.history.trim_to_bytes_with(bytes, held_bytes);
        }
    }

    /// The object a member observes for a recorded structure, even out of focus.
    pub fn object_for(&self, member: u32, structure: StructureId) -> Option<Object> {
        self.objects.iter().find_map(|(object, binding)| {
            (object.member == member && binding.structure == structure).then_some(*object)
        })
    }

    /// Sources retained by all objects observed by one member, across orders.
    pub fn sources_for(&self, member: u32) -> Vec<SourceId> {
        self.objects
            .iter()
            .filter(|(object, _)| object.member == member)
            .flat_map(|(_, binding)| {
                self.orders[&binding.scope]
                    .history
                    .held_sources_for(binding.structure)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Releases across orders; sources still held elsewhere remain pending.
    pub fn released_sources(&mut self) -> Vec<SourceId> {
        for order in self.orders.values_mut() {
            self.released.extend(order.history.released_sources());
        }
        let held: BTreeSet<_> = self.held_sources().into_iter().collect();
        let ready: Vec<_> = self.released.difference(&held).copied().collect();
        for source in &ready {
            self.released.remove(source);
        }
        ready
    }
}

impl Deref for Histories {
    type Target = History;
    fn deref(&self) -> &History {
        &self.orders[&self.selected].history
    }
}

impl DerefMut for Histories {
    fn deref_mut(&mut self) -> &mut History {
        &mut self
            .orders
            .get_mut(&self.selected)
            .expect("selected order")
            .history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::history::{Entry, Step};
    use serde_json::json;

    fn record(histories: &mut Histories, structure: StructureId, label: &str, source: u64) {
        histories.record(
            Entry::new(
                label,
                structure,
                Step::Edit(clausters_document::Opaque(json!({"value": 1}))),
                clausters_document::Opaque(json!({"value": 0})),
            )
            .holding(vec![SourceId(source)], vec![SourceId(source)]),
        );
    }

    #[test]
    fn domains_share_an_order_by_membership_and_extra_objects_are_independent() {
        let mut histories = Histories::default();
        let a = Object { member: 0, id: 1 };
        let b = Object { member: 1, id: 0 };
        let a_structure = histories.join(a, "a", "samples");
        let b_structure = histories.join(b, "b", "points");
        record(&mut histories, a_structure, "a", 7);
        record(&mut histories, b_structure, "b", 8);
        let extra = Object { member: 0, id: 2 };
        let structure = histories.focus(extra, "samples");
        record(&mut histories, structure, "extra", 9);
        assert_eq!(histories.undo_label().as_deref(), Some("extra"));
        histories.focus(a, "samples");
        assert_eq!(histories.undo_label().as_deref(), Some("b"));
        histories.undo();
        assert_eq!(histories.undo_label().as_deref(), Some("a"));
        histories.focus(extra, "samples");
        assert_eq!(histories.undo_label().as_deref(), Some("extra"));
        assert_eq!(
            histories.held_sources(),
            [SourceId(7), SourceId(8), SourceId(9)]
        );
        assert_eq!(histories.sources_for(0), [SourceId(7), SourceId(9)]);
    }

    #[test]
    fn closing_the_last_observer_releases_inactive_history_without_reusing_identity() {
        let mut histories = Histories::new();
        let a = Object { member: 0, id: 1 };
        let b = Object { member: 1, id: 1 };
        let structure = histories.join(a, "same", "samples");
        assert_eq!(histories.join(b, "same", "samples"), structure);
        record(&mut histories, structure, "shared", 7);
        histories.close(a);
        assert!(histories.can_undo());
        let extra = Object { member: 1, id: 2 };
        let independent = histories.focus(extra, "samples");
        record(&mut histories, independent, "independent", 8);
        histories.focus(b, "samples");
        histories.close(extra);
        assert_eq!(histories.released_sources(), [SourceId(8)]);
        histories.close(b);
        histories.clear();
        assert_eq!(histories.released_sources(), [SourceId(7)]);
        assert_ne!(histories.join(a, "same", "samples"), structure);
    }
}
