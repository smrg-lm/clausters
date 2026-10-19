//! JSON adapter for the typed clipboard service. Host event layout belongs here.
use super::*;
use clausters_document::clipboard::Clipboard;

impl Editing {
    pub(super) fn exchange_json(&mut self, request: &Value) -> Value {
        let server = serde_json::from_value(request["server"].clone()).unwrap_or_default();
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
                    let doc =
                        Clipboard::parse(args.get(6).and_then(Value::as_str).unwrap_or_default());
                    match self
                        .services
                        .clipboard
                        .acquire(&mut self.services.resources, doc, server)
                    {
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
            "publish" => {
                match serde_json::from_str::<Clipboard>(
                    request["document"].as_str().unwrap_or_default(),
                ) {
                    Ok(doc) => match self.services.clipboard.publish(
                        &mut self.services.resources,
                        doc,
                        server,
                        request["member"].as_u64().unwrap_or(0) as u32,
                    ) {
                        Ok(document) => json!({"document":document}),
                        Err(error) => json!({"error":error}),
                    },
                    Err(_) => json!({"error":"invalid clipboard document"}),
                }
            }
            "acquire" => match self.services.clipboard.acquire(
                &mut self.services.resources,
                Clipboard::parse(request["document"].as_str().unwrap_or_default()),
                server,
            ) {
                Ok(lease) => json!({"lease":lease}),
                Err(error) => json!({"error":error}),
            },
            "release" => {
                if let Some(lease) = request["lease"].as_u64() {
                    self.services
                        .clipboard
                        .release(&mut self.services.resources, lease);
                }
                json!({})
            }
            "clear" => {
                self.services.clipboard.clear(&mut self.services.resources);
                json!({})
            }
            "invalidate" => {
                self.invalidate_server(server);
                self.services.clipboard.clear(&mut self.services.resources);
                json!({})
            }
            _ => json!({"error":"unknown clipboard exchange action"}),
        };
        let released = self.services.resources.collect();
        for source in &released {
            for seat in &mut self.seats {
                if seat.server == source.id.server
                    && let Member::Audio(editor) = &mut seat.member
                {
                    editor.forget(source.id.buffer);
                }
            }
        }
        result["released"] = json!(
            released
                .into_iter()
                .map(|source| source.id)
                .collect::<Vec<_>>()
        );
        result
    }
}
