//! Immutable clipboard publications and temporary readers over resource leases.
//!
//! A composition supplies one exchange per endpoint incarnation, independent
//! of editor lifetimes. Adapters execute I/O; this service decides publication,
//! replacement and whether a remote reference is still usable.

use clausters_document::clipboard::{BufferLocation, Clipboard, Content};

use super::resources::{ResourceId, Resources, Root, RootKind, ServerInstance, Source};

/// A clipboard exchange, usable without a renderer or an open document.
#[derive(Debug)]
pub struct Exchange {
    owner: u64,
    readers: std::collections::BTreeMap<u64, Root>,
    published: Option<Clipboard>,
    next: u64,
}

impl Exchange {
    /// A clipboard consumer with its own composition-scoped owner identity.
    pub fn new(owner: u64) -> Self {
        Self {
            owner,
            readers: std::collections::BTreeMap::new(),
            published: None,
            next: 0,
        }
    }

    fn root(kind: RootKind, owner: u64) -> Root {
        Root { kind, owner }
    }

    /// Publish immutable buffer contents after their producing work completes.
    pub fn publish(
        &mut self,
        resources: &mut Resources,
        document: Clipboard,
        server: ServerInstance,
        member: u32,
    ) -> Result<String, String> {
        let mut doc = document;
        let Content::Buffer {
            channels,
            frames,
            sample_rate,
            buffer,
            location,
        } = &mut doc.content
        else {
            return Err("a remote publication needs a buffer".into());
        };
        if *buffer < 0
            || *channels == 0
            || *frames == 0
            || !sample_rate.is_finite()
            || *sample_rate <= 0.0
        {
            return Err("invalid copied buffer shape or rate".into());
        }
        let bytes = frames
            .checked_mul(u64::from(*channels))
            .and_then(|n| n.checked_mul(4))
            .ok_or("copied buffer is too large")?;
        let id = ResourceId {
            server,
            buffer: i64::from(*buffer),
        };
        if !resources.valid(server) || self.published.as_ref().is_some_and(|doc| matches!(&doc.content, Content::Buffer { buffer, .. } if i64::from(*buffer) == id.buffer)) {
            return Err("a publication needs a fresh buffer on a live instance".into());
        }
        self.next += 1;
        *location = Some(BufferLocation {
            connection: server.connection,
            generation: server.generation,
            publication: clausters_core::rng::splitmix64(
                clausters_core::rng::entropy_seed() ^ self.next,
            ) & ((1u64 << 52) - 1),
        });
        if resources.source(id).is_some_and(|source| {
            source.bytes != bytes || !source.owned || resources.retained_as(id, RootKind::Contents)
        }) {
            return Err("the publication does not match its owned source".into());
        }
        if resources.source(id).is_none() {
            resources.register(Source {
                id,
                member,
                bytes,
                owned: true,
                stored: None,
            });
        }
        resources.retain(Self::root(RootKind::Clipboard, self.owner), [id]);
        let raw = doc.to_json();
        self.published = Some(doc);
        Ok(raw)
    }

    /// Retain one publication for a reader on the same server incarnation.
    pub fn acquire(
        &mut self,
        resources: &mut Resources,
        doc: Clipboard,
        server: ServerInstance,
    ) -> Result<Option<u64>, String> {
        let Content::Buffer {
            buffer, location, ..
        } = &doc.content
        else {
            // Reconcile external text/typed replacement at the next exchange.
            self.clear(resources);
            return Ok(None);
        };
        let location = location.ok_or("the clipboard buffer has no owning exchange")?;
        if location.connection != server.connection || location.generation != server.generation {
            return Err("the clipboard buffer belongs to another server instance; cross-server transfer is not supported".into());
        }
        if self.published.as_ref() != Some(&doc) {
            return Err("the clipboard buffer publication is no longer available".into());
        }
        let lease = resources.new_owner();
        if !resources.retain(
            Self::root(RootKind::Pending, lease),
            [ResourceId {
                server,
                buffer: i64::from(*buffer),
            }],
        ) {
            return Err("the clipboard server instance is no longer available".into());
        }
        self.readers
            .insert(lease, Self::root(RootKind::Pending, lease));
        Ok(Some(lease))
    }

    /// End a reader without interpreting an editor action or host event.
    pub fn release(&mut self, resources: &mut Resources, lease: u64) {
        if let Some(root) = self.readers.remove(&lease) {
            resources.drop_root(root);
        }
    }

    /// End this publication while retaining independent readers.
    pub fn clear(&mut self, resources: &mut Resources) {
        self.published = None;
        resources.drop_root(Self::root(RootKind::Clipboard, self.owner));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(buffer: i64) -> Clipboard {
        serde_json::from_value(json!({"content":{"kind":"buffer","buffer":buffer,"channels":1,"frames":4,"sample_rate":48000}})).unwrap()
    }
    fn server() -> ServerInstance {
        ServerInstance {
            connection: 1,
            generation: 1,
        }
    }

    #[test]
    fn publishing_mutable_contents_preserves_the_previous_clipboard() {
        let mut resources = Resources::default();
        let mut exchange = Exchange::new(resources.new_owner());
        let previous = exchange
            .publish(&mut resources, doc(20), server(), 7)
            .unwrap();
        let id = ResourceId {
            server: server(),
            buffer: 30,
        };
        resources.register(Source {
            id,
            member: 7,
            bytes: 16,
            owned: true,
            stored: None,
        });
        let contents = Root {
            kind: RootKind::Contents,
            owner: 7,
        };
        resources.retain(contents, [id]);
        assert!(
            exchange
                .publish(&mut resources, doc(30), server(), 7)
                .is_err()
        );
        assert!(
            exchange
                .acquire(&mut resources, Clipboard::parse(&previous), server())
                .unwrap()
                .is_some()
        );
        resources.drop_root(contents);
        assert!(
            exchange
                .publish(&mut resources, doc(30), server(), 7)
                .is_ok()
        );
    }

    #[test]
    fn clipboard_and_history_retain_the_same_source_in_one_graph() {
        let mut resources = Resources::default();
        let mut exchange = Exchange::new(resources.new_owner());
        let raw = exchange
            .publish(&mut resources, doc(20), server(), 7)
            .unwrap();
        let id = ResourceId {
            server: server(),
            buffer: 20,
        };
        let history = Root {
            kind: RootKind::History,
            owner: 2,
        };
        assert!(resources.retain(history, [id]));
        let lease = exchange
            .acquire(&mut resources, Clipboard::parse(&raw), server())
            .unwrap()
            .unwrap();
        exchange.clear(&mut resources);
        assert!(resources.collect().is_empty());
        exchange.release(&mut resources, lease);
        assert!(resources.collect().is_empty());
        resources.drop_root(history);
        assert_eq!(resources.collect()[0].member, 7);
        assert!(resources.collect().is_empty());
    }

    #[test]
    fn replacement_readers_and_foreign_incarnations_are_independent() {
        let mut resources = Resources::default();
        let mut a = Exchange::new(resources.new_owner());
        let mut b = Exchange::new(resources.new_owner());
        let raw = a.publish(&mut resources, doc(20), server(), 1).unwrap();
        b.publish(&mut resources, doc(21), server(), 2).unwrap();
        assert!(
            a.acquire(
                &mut resources,
                Clipboard::parse(&raw),
                ServerInstance {
                    generation: 2,
                    ..server()
                }
            )
            .is_err()
        );
        let lease = a
            .acquire(&mut resources, Clipboard::parse(&raw), server())
            .unwrap()
            .unwrap();
        a.publish(&mut resources, doc(22), server(), 1).unwrap();
        assert!(resources.collect().is_empty());
        a.release(&mut resources, lease);
        assert_eq!(resources.collect()[0].id.buffer, 20);
        a.clear(&mut resources);
        assert_eq!(resources.collect()[0].id.buffer, 22);
        assert!(resources.retained(ResourceId {
            server: server(),
            buffer: 21
        }));
    }
}
