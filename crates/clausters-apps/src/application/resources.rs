//! Source ownership shared by application services, without filesystem I/O.
//!
//! A root is replaced atomically, and an unreferenced owned source is
//! released once.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

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
}

/// One exact release attempt, independent of a recycled buffer number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTicket {
    /// Composition-scoped release identity.
    pub id: u64,
    /// Original endpoint and buffer.
    pub resource: ResourceId,
}

/// Server work required before an allocator id can be returned.
#[derive(Debug, Serialize)]
pub struct Release {
    /// Echoed after the waits settle.
    pub ticket: ReleaseTicket,
    /// Endpoint owner, even after its document closes.
    pub member: u32,
    /// Existing Step encoding for the executor.
    pub steps: serde_json::Value,
}

#[derive(Debug)]
struct Retiring {
    ticket: ReleaseTicket,
    source: Source,
    waiting: bool,
    reason: Option<String>,
}

/// Shared source ownership and the byte budget's accounting.
#[derive(Debug, Default)]
pub struct Resources {
    next_owner: u64,
    retiring: BTreeMap<ResourceId, Retiring>,
    sources: BTreeMap<ResourceId, Source>,
    roots: BTreeMap<Root, BTreeSet<ResourceId>>,
    invalid: BTreeSet<ServerInstance>,
    order: Vec<ResourceId>,
}

impl Resources {
    /// Reserve a consumer identity shared by all services in this composition.
    pub fn new_owner(&mut self) -> u64 {
        self.next_owner = self
            .next_owner
            .checked_add(1)
            .expect("resource owner identity exhausted");
        self.next_owner
    }

    /// Registered locations and owners, for a storage port or diagnostics.
    pub fn sources(&self) -> Vec<Source> {
        self.sources.values().cloned().collect()
    }

    /// Whether this connection incarnation is still usable.
    pub fn valid(&self, server: ServerInstance) -> bool {
        !self.invalid.contains(&server)
    }

    /// Whether a new allocation may reserve this remote id.
    pub fn available(&self, id: ResourceId) -> bool {
        self.valid(id.server) && !self.sources.contains_key(&id) && !self.retiring.contains_key(&id)
    }

    /// Register metadata. Invalidated instances cannot regain resources.
    pub fn register(&mut self, mut source: Source) -> bool {
        if self.invalid.contains(&source.id.server) || self.retiring.contains_key(&source.id) {
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

    /// Whether a source is retained for this particular reason.
    pub fn retained_as(&self, id: ResourceId, kind: RootKind) -> bool {
        self.roots
            .iter()
            .any(|(root, sources)| root.kind == kind && sources.contains(&id))
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

    /// Transfer a close effect's unretained source to acknowledged cleanup.
    pub fn released(&mut self, id: ResourceId) -> bool {
        if self.retained(id) {
            return false;
        }
        if let Some(source) = self.sources.remove(&id).filter(|source| source.owned) {
            self.retire(source);
        }
        self.order.retain(|source| *source != id);
        true
    }

    fn retire(&mut self, source: Source) {
        let ticket = ReleaseTicket {
            id: self.new_owner(),
            resource: source.id,
        };
        self.retiring.insert(
            source.id,
            Retiring {
                ticket,
                source,
                waiting: false,
                reason: None,
            },
        );
    }

    /// Dispatch cleanup once. Failed attempts may be retried with the same
    /// ticket; ids cannot be registered or acquired while they are retiring.
    pub fn begin_release(&mut self, id: ResourceId) -> Option<Release> {
        let retiring = self.retiring.get_mut(&id)?;
        if retiring.waiting {
            return None;
        }
        retiring.waiting = true;
        let steps = {
            use clausters_core::osc::{OscMessage, OscType};
            use clausters_editing::apply::{Step, steps_json};
            steps_json(&[
                Step::Send(OscMessage {
                    addr: "/buffer_free".into(),
                    args: vec![OscType::Int(id.buffer as i32)],
                }),
                Step::AwaitDone {
                    command: "/buffer_free".into(),
                    index: Some(id.buffer as i32),
                },
            ])
        };
        Some(Release {
            ticket: retiring.ticket,
            member: retiring.source.member,
            steps,
        })
    }

    /// Confirm exactly one dispatched release. Failure keeps the source and
    /// reservation retryable; a duplicate result cannot return an id twice.
    pub fn finish_release(&mut self, ticket: ReleaseTicket, result: Result<(), String>) -> bool {
        let Some(retiring) = self.retiring.get_mut(&ticket.resource) else {
            return false;
        };
        if retiring.ticket != ticket || !retiring.waiting {
            return false;
        }
        if let Err(reason) = result {
            retiring.waiting = false;
            retiring.reason = Some(reason);
            return false;
        }
        self.retiring.remove(&ticket.resource);
        true
    }

    /// Unsettled cleanup identities for an executor that is reconnecting or
    /// retrying. This is metadata, never restored sample payloads.
    pub fn pending_releases(&self) -> Vec<ReleaseTicket> {
        self.retiring
            .values()
            .filter(|release| !release.waiting)
            .map(|release| release.ticket)
            .collect()
    }

    /// The last cleanup refusal, retained until a successful retry or invalidation.
    pub fn release_error(&self, ticket: ReleaseTicket) -> Option<&str> {
        self.retiring
            .get(&ticket.resource)
            .filter(|release| release.ticket == ticket)?
            .reason
            .as_deref()
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
                self.retire(source.clone());
                out.push(source);
            }
        }
        self.order.retain(|id| self.sources.contains_key(id));
        out
    }

    /// Forget a lost instance without sending frees to its replacement.
    /// The returned ids let consumers cancel work and discard stale bindings.
    pub fn invalidate(&mut self, server: ServerInstance) -> Vec<ResourceId> {
        self.invalid.insert(server);
        self.retiring.retain(|id, _| id.server != server);
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
        }
    }
    fn root(kind: RootKind) -> Root {
        Root { kind, owner: 1 }
    }

    #[test]
    fn cleanup_failure_keeps_ids_reserved_until_the_exact_successful_retry() {
        let mut resources = Resources::default();
        let source = Source {
            id: ResourceId {
                server: ServerInstance {
                    connection: 1,
                    generation: 1,
                },
                buffer: 7,
            },
            member: 2,
            bytes: 16,
            owned: true,
        };
        resources.register(source.clone());
        assert_eq!(
            resources.collect().as_slice(),
            std::slice::from_ref(&source)
        );
        let release = resources.begin_release(source.id).unwrap();
        assert!(resources.begin_release(source.id).is_none());
        assert!(
            !resources.register(source.clone()),
            "an unsettled free cannot race reuse"
        );
        assert!(!resources.finish_release(release.ticket, Err("offline".into())));
        assert_eq!(resources.release_error(release.ticket), Some("offline"));
        assert_eq!(resources.pending_releases(), [release.ticket]);
        let retry = resources.begin_release(source.id).unwrap();
        assert_eq!(retry.ticket, release.ticket);
        assert!(resources.finish_release(retry.ticket, Ok(())));
        assert!(!resources.finish_release(retry.ticket, Ok(())));
        assert!(resources.register(source.clone()));
        resources.collect();
        let next = resources.begin_release(source.id).unwrap();
        assert_ne!(next.ticket, retry.ticket);
        assert!(!resources.finish_release(retry.ticket, Ok(())));
        resources.invalidate(source.id.server);
        assert!(resources.pending_releases().is_empty());
        assert!(!resources.finish_release(next.ticket, Ok(())));
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
