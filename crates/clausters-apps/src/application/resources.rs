//! Source ownership shared by application services, without filesystem I/O.
//!
//! A root is replaced atomically. Only history-only sources may spill; an
//! unreferenced owned source is released once. Endpoints execute storage work
//! through their existing disk/OPFS buffer commands.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// A connection incarnation, distinct even when its endpoint and ids repeat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ServerInstance {
    /// The connection's identity.
    pub connection: u64,
    /// Its incarnation, advanced on reconnect or restart.
    pub generation: u64,
}

/// A remote source; a local buffer number alone is never an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResourceId {
    /// The instance owning the buffer.
    pub server: ServerInstance,
    /// Its local buffer number.
    pub buffer: i64,
}

/// Independent reasons for retaining a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RootKind {
    /// A document still reads it.
    Contents,
    /// Undo or redo can read it.
    History,
    /// A published clipboard snapshot owns it.
    Clipboard,
    /// A playback graph reads it.
    Playback,
    /// An operation has not finished with it.
    Pending,
}

/// One consumer's ownership, independent of all other consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Root {
    /// Which service retains these sources.
    pub kind: RootKind,
    /// A stable consumer number, allocated by the composition.
    pub owner: u64,
}

/// Metadata needed to budget and release a retained source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The remote identity.
    pub id: ResourceId,
    /// The member whose endpoint executes release and storage.
    pub member: u32,
    /// Resident sample bytes.
    pub bytes: u64,
    /// Whether this composition may free it.
    pub owned: bool,
    /// Its scratch path, when its buffer has been spilled.
    pub stored: Option<String>,
}

/// A scratch namespace owned by one composition, on disk or OPFS.
#[derive(Clone, Debug)]
pub struct ScratchScope {
    /// The endpoint incarnation whose samples are stored.
    pub server: ServerInstance,
    consumer: u64,
}

impl Default for ScratchScope {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            server: ServerInstance::default(),
            consumer: clausters_core::rng::splitmix64(
                clausters_core::rng::entropy_seed() ^ NEXT.fetch_add(1, Ordering::Relaxed),
            ),
        }
    }
}

impl ScratchScope {
    /// A name scoped to the endpoint and composition in its scratch directory.
    pub fn path(&self, directory: &str, buffer: i64) -> String {
        format!(
            "{}/take-{}-{}-{}-{buffer}.wav",
            directory.trim_end_matches('/'),
            self.server.connection,
            self.server.generation,
            self.consumer
        )
    }
}

/// Shared source ownership and resident-budget policy.
#[derive(Debug, Default)]
pub struct Resources {
    sources: BTreeMap<ResourceId, Source>,
    roots: BTreeMap<Root, BTreeSet<ResourceId>>,
    invalid: BTreeSet<ServerInstance>,
    order: Vec<ResourceId>,
}

impl Resources {
    /// Registered locations and owners, for a storage port or diagnostics.
    pub fn sources(&self) -> Vec<Source> {
        self.sources.values().cloned().collect()
    }

    /// Whether this connection incarnation is still usable.
    pub fn valid(&self, server: ServerInstance) -> bool {
        !self.invalid.contains(&server)
    }

    /// Register metadata. Invalidated instances cannot regain resources.
    pub fn register(&mut self, mut source: Source) -> bool {
        if self.invalid.contains(&source.id.server) {
            return false;
        }
        if let Some(previous) = self.sources.get(&source.id) {
            if previous.owned {
                source.owned = true;
                source.member = previous.member;
            }
        } else {
            self.order.push(source.id);
        }
        self.sources.insert(source.id, source);
        true
    }

    /// Replace one consumer's roots without changing any other consumer.
    /// Unknown or invalid references refuse the whole replacement.
    pub fn retain(&mut self, root: Root, sources: impl IntoIterator<Item = ResourceId>) -> bool {
        let sources: BTreeSet<_> = sources.into_iter().collect();
        if sources
            .iter()
            .any(|s| !self.sources.contains_key(s) || self.invalid.contains(&s.server))
        {
            return false;
        }
        self.roots.insert(root, sources);
        true
    }

    /// End one consumer's lease. Collection remains an explicit boundary.
    pub fn drop_root(&mut self, root: Root) {
        self.roots.remove(&root);
    }

    /// Whether any independent consumer still holds a source.
    pub fn retained(&self, id: ResourceId) -> bool {
        self.roots.values().any(|sources| sources.contains(&id))
    }

    /// Whether a consumer outside the history still needs this source.
    pub fn retained_outside_history(&self, id: ResourceId) -> bool {
        self.roots
            .iter()
            .any(|(root, sources)| root.kind != RootKind::History && sources.contains(&id))
    }

    /// Read a registered source's current location and ownership.
    pub fn source(&self, id: ResourceId) -> Option<&Source> {
        self.sources.get(&id)
    }

    /// Remove an unretained source whose release is already in a close effect.
    pub fn released(&mut self, id: ResourceId) -> bool {
        if self.retained(id) {
            return false;
        }
        self.sources.remove(&id);
        self.order.retain(|source| *source != id);
        true
    }

    /// Owned sources no consumer can reach, reported once, in creation order.
    pub fn collect(&mut self) -> Vec<Source> {
        let free: Vec<_> = self
            .order
            .iter()
            .copied()
            .filter(|id| !self.retained(*id))
            .collect();
        let mut out = Vec::new();
        for id in free {
            if let Some(source) = self.sources.remove(&id).filter(|s| s.owned) {
                out.push(source);
            }
        }
        self.order.retain(|id| self.sources.contains_key(id));
        out
    }

    /// Oldest resident sources held only by history, until they fit the limit.
    /// `available` states which sources have a usable storage port.
    /// This reserves nothing; the caller publishes a stored location after
    /// execution or leaves the source resident on failure.
    pub fn spill_candidates(
        &self,
        limit: u64,
        available: impl Fn(ResourceId) -> bool,
    ) -> Vec<ResourceId> {
        let eligible: Vec<_> = self
            .order
            .iter()
            .copied()
            .filter(|id| {
                let source = &self.sources[id];
                source.owned
                    && source.stored.is_none()
                    && self.retained(*id)
                    && self.roots.iter().all(|(root, sources)| {
                        root.kind == RootKind::History || !sources.contains(id)
                    })
            })
            .collect();
        let mut total: u64 = eligible.iter().map(|id| self.sources[id].bytes).sum();
        eligible
            .into_iter()
            .filter(|id| available(*id))
            .take_while(|id| {
                if total <= limit {
                    return false;
                }
                total = total.saturating_sub(self.sources[id].bytes);
                true
            })
            .collect()
    }

    /// Confirm a spill or restore after the endpoint succeeds.
    pub fn location(&mut self, id: ResourceId, path: Option<String>) -> bool {
        let Some(source) = self.sources.get_mut(&id) else {
            return false;
        };
        source.stored = path;
        true
    }

    /// Forget a lost instance without sending frees to its replacement.
    /// The returned ids let consumers cancel work and discard stale bindings.
    pub fn invalidate(&mut self, server: ServerInstance) -> Vec<ResourceId> {
        self.invalid.insert(server);
        let removed: Vec<_> = self
            .sources
            .keys()
            .copied()
            .filter(|id| id.server == server)
            .collect();
        for id in &removed {
            self.sources.remove(id);
        }
        for sources in self.roots.values_mut() {
            sources.retain(|id| id.server != server);
        }
        self.order.retain(|id| id.server != server);
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(connection: u64, buffer: i64) -> Source {
        Source {
            id: ResourceId {
                server: ServerInstance {
                    connection,
                    generation: 1,
                },
                buffer,
            },
            member: 0,
            bytes: 16,
            owned: true,
            stored: None,
        }
    }
    fn root(kind: RootKind) -> Root {
        Root { kind, owner: 1 }
    }

    #[test]
    fn every_consumer_independently_retains_and_releases_the_same_source() {
        let mut resources = Resources::default();
        let source = source(1, 7);
        let id = source.id;
        resources.register(source);
        let kinds = [
            RootKind::Contents,
            RootKind::History,
            RootKind::Clipboard,
            RootKind::Playback,
            RootKind::Pending,
        ];
        for kind in kinds {
            assert!(resources.retain(root(kind), [id]));
        }
        for kind in kinds {
            assert!(resources.collect().is_empty());
            resources.drop_root(root(kind));
        }
        assert_eq!(resources.collect().len(), 1);
        assert!(resources.collect().is_empty());
    }

    #[test]
    fn replacement_and_failed_acquisition_preserve_other_consumers() {
        let mut resources = Resources::default();
        let first = source(1, 7);
        let second = source(1, 8);
        resources.register(first.clone());
        resources.register(second.clone());
        resources.retain(root(RootKind::Clipboard), [first.id]);
        resources.retain(root(RootKind::Pending), [first.id]);
        assert!(!resources.retain(root(RootKind::Pending), [source(2, 7).id]));
        resources.retain(root(RootKind::Clipboard), [second.id]);
        assert!(resources.collect().is_empty());
        resources.drop_root(root(RootKind::Pending));
        assert_eq!(resources.collect(), [first]);
        assert!(resources.collect().is_empty());
    }

    #[test]
    fn only_history_only_owned_sources_spill_and_failures_stay_resident() {
        let mut resources = Resources::default();
        let first = source(1, 7);
        let mut second = source(1, 8);
        second.owned = false;
        resources.register(first.clone());
        resources.register(second.clone());
        resources.retain(root(RootKind::History), [first.id, second.id]);
        resources.retain(root(RootKind::Playback), [first.id]);
        assert!(resources.spill_candidates(0, |_| true).is_empty());
        resources.drop_root(root(RootKind::Playback));
        assert_eq!(resources.spill_candidates(0, |_| true), [first.id]);
        assert_eq!(
            resources.spill_candidates(0, |_| true),
            [first.id],
            "a refused write changes no location"
        );
        resources.location(first.id, Some("scratch/take-7.wav".into()));
        assert!(resources.spill_candidates(0, |_| true).is_empty());
        resources.location(first.id, None);
        assert_eq!(resources.spill_candidates(0, |_| true), [first.id]);
    }

    #[test]
    fn a_borrowing_observer_does_not_replace_the_sources_release_owner() {
        let mut resources = Resources::default();
        let original = source(1, 7);
        resources.register(original.clone());
        let mut borrowed = original.clone();
        borrowed.owned = false;
        borrowed.member = 9;
        resources.register(borrowed);
        assert_eq!(resources.collect(), [original]);
    }

    #[test]
    fn a_source_without_storage_does_not_block_other_spill_candidates() {
        let mut resources = Resources::default();
        let first = source(1, 7);
        let second = source(1, 8);
        resources.register(first.clone());
        resources.register(second.clone());
        resources.retain(root(RootKind::History), [first.id, second.id]);
        assert_eq!(
            resources.spill_candidates(16, |id| id != first.id),
            [second.id]
        );
    }

    #[test]
    fn server_incarnations_disambiguate_ids_and_invalidation_sends_no_frees() {
        let mut resources = Resources::default();
        let old = source(1, 7);
        let other = source(2, 7);
        let mut replacement = old.clone();
        replacement.id.server.generation += 1;
        for source in [&old, &other, &replacement] {
            assert!(resources.register(source.clone()));
        }
        resources.retain(
            root(RootKind::Clipboard),
            [old.id, other.id, replacement.id],
        );
        assert_eq!(resources.invalidate(old.id.server), [old.id]);
        assert!(!resources.register(old.clone()));
        assert!(!resources.retain(root(RootKind::Pending), [old.id]));
        assert!(resources.collect().is_empty());
        resources.drop_root(root(RootKind::Clipboard));
        assert_eq!(resources.collect(), [other, replacement]);
    }
}
