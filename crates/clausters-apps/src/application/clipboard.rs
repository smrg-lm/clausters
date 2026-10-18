//! Immutable clipboard publications and temporary readers over resource leases.
//!
//! A composition supplies one exchange per endpoint incarnation, independent
//! of editor lifetimes. Adapters execute I/O; this service decides publication,
//! replacement and whether a remote reference is still usable.

use clausters_document::clipboard::{BufferLocation, Clipboard, Content};
use serde_json::{Value, json};

use super::resources::{ResourceId, Resources, Root, RootKind, ServerInstance, Source};

/// A clipboard exchange, usable without a renderer or an open document.
#[derive(Debug, Default)]
pub struct Exchange {
    resources: Resources,
    published: Option<Clipboard>,
    next: u64,
}

impl Exchange {
    fn root(kind: RootKind, owner: u64) -> Root {
        Root { kind, owner }
    }

    fn publish(&mut self, raw: &str, server: ServerInstance) -> Result<String, String> {
        let mut doc: Clipboard =
            serde_json::from_str(raw).map_err(|_| "invalid clipboard document")?;
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
        if !self.resources.valid(server) || self.resources.source(id).is_some() {
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
        self.resources.register(Source {
            id,
            member: 0,
            bytes,
            owned: true,
            stored: None,
        });
        self.resources
            .retain(Self::root(RootKind::Clipboard, 0), [id]);
        let raw = doc.to_json();
        self.published = Some(doc);
        Ok(raw)
    }

    fn acquire(&mut self, raw: &str, server: ServerInstance) -> Result<Option<u64>, String> {
        let doc = Clipboard::parse(raw);
        let Content::Buffer {
            buffer, location, ..
        } = &doc.content
        else {
            // Reconcile external text/typed replacement at the next exchange.
            self.clear();
            return Ok(None);
        };
        let location = location.ok_or("the clipboard buffer has no owning exchange")?;
        if location.connection != server.connection || location.generation != server.generation {
            return Err("the clipboard buffer belongs to another server instance; cross-server transfer is not supported".into());
        }
        if self.published.as_ref() != Some(&doc) {
            return Err("the clipboard buffer publication is no longer available".into());
        }
        self.next += 1;
        let lease = self.next;
        if !self.resources.retain(
            Self::root(RootKind::Pending, lease),
            [ResourceId {
                server,
                buffer: i64::from(*buffer),
            }],
        ) {
            return Err("the clipboard server instance is no longer available".into());
        }
        Ok(Some(lease))
    }

    fn clear(&mut self) {
        self.published = None;
        self.resources.drop_root(Self::root(RootKind::Clipboard, 0));
    }

    /// Internal JSON port through the existing editing door. `publish` is
    /// called only after copy steps succeeded; `acquire` brackets paste I/O.
    /// `release` ends a reader even on failure. `clear` reconciles replacement;
    /// `invalidate` forgets a lost endpoint without freeing on its replacement.
    pub fn call(&mut self, request: &Value) -> Value {
        let server = serde_json::from_value(request["server"].clone()).unwrap_or_default();
        let raw = request["document"].as_str().unwrap_or_default();
        let mut result = match request["action"].as_str().unwrap_or_default() {
            "event" => {
                let mut args = request["args"].as_array().cloned().unwrap_or_default();
                let tag = args
                    .get(3)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if tag == "cut" {
                    args[3] = json!("copy_cut");
                }
                let mut result = json!({});
                if matches!(tag.as_str(), "paste" | "mix") {
                    let raw = args.get(6).and_then(Value::as_str).unwrap_or_default();
                    match self.acquire(raw, server) {
                        Ok(lease) => result["lease"] = json!(lease),
                        Err(error) => {
                            args.truncate(3);
                            args.extend([json!("refused"), json!(tag), json!(error)]);
                        }
                    }
                }
                result["args"] = json!(args);
                result
            }
            "publish" => match self.publish(raw, server) {
                Ok(document) => json!({"document": document}),
                Err(error) => json!({"error": error}),
            },
            "acquire" => match self.acquire(raw, server) {
                Ok(lease) => json!({"lease": lease}),
                Err(error) => json!({"error": error}),
            },
            "release" => {
                if let Some(lease) = request["lease"].as_u64() {
                    self.resources
                        .drop_root(Self::root(RootKind::Pending, lease));
                }
                json!({})
            }
            "clear" => {
                self.clear();
                json!({})
            }
            "invalidate" => {
                self.resources.invalidate(server);
                self.published = None;
                json!({})
            }
            _ => json!({"error": "unknown clipboard exchange action"}),
        };
        result["released"] = json!(
            self.resources
                .collect()
                .into_iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(exchange: &mut Exchange, buffer: i64) -> Value {
        exchange.call(&json!({"action":"publish", "server":{"connection":1,"generation":1},
            "document":json!({"content":{"kind":"buffer","buffer":buffer,"channels":1,"frames":4,"sample_rate":48000}}).to_string()}))
    }

    #[test]
    fn replacement_waits_for_readers_and_stale_references_refuse() {
        let mut exchange = Exchange::default();
        let first = copy(&mut exchange, 20);
        let acquired = exchange.call(&json!({"action":"acquire","server":{"connection":1,"generation":1},"document":first["document"]}));
        let second = copy(&mut exchange, 21);
        assert_eq!(second["released"], json!([]));
        let stale = exchange.call(&json!({"action":"acquire","server":{"connection":1,"generation":1},"document":first["document"]}));
        assert!(stale["error"].is_string());
        let release = exchange.call(&json!({"action":"release","lease":acquired["lease"]}));
        assert_eq!(release["released"][0]["buffer"], 20);
        assert_eq!(
            exchange.call(&json!({"action":"release","lease":acquired["lease"]}))["released"],
            json!([])
        );
    }

    #[test]
    fn text_replacement_releases_and_foreign_incarnations_refuse() {
        let mut exchange = Exchange::default();
        let first = copy(&mut exchange, 20);
        for server in [
            json!({"connection":2,"generation":1}),
            json!({"connection":1,"generation":2}),
        ] {
            let refused = exchange
                .call(&json!({"action":"acquire","server":server,"document":first["document"]}));
            assert!(refused["error"].is_string());
            assert_eq!(refused["released"], json!([]));
        }
        let text = exchange.call(&json!({"action":"acquire","document":"external text"}));
        assert_eq!(text["released"][0]["buffer"], 20);
    }

    #[test]
    fn invalidation_does_not_release_buffers_on_a_replacement() {
        let mut exchange = Exchange::default();
        copy(&mut exchange, 20);
        assert_eq!(
            exchange.call(&json!({"action":"invalidate","server":{"connection":1,"generation":1}}))
                ["released"],
            json!([])
        );
        assert!(copy(&mut exchange, 21)["error"].is_string());
    }
}
