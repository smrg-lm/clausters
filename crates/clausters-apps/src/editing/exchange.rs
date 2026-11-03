//! **The clipboard's exchange**: a gesture's clipboard lease taken, a copy
//! published, a lease let go -- typed calls of the context, made by the
//! application that holds the editor.
use super::*;
use clausters_document::clipboard::Clipboard;

impl Editing {
    /// **A gesture prepared against the clipboard**: a paste or a mix takes
    /// a lease on the document it reads (`args[6]`), or is refused in its own
    /// arguments when it cannot; a cut is a copy until its copy is
    /// published. Answers the arguments the member reads, and the lease
    /// taken, if any.
    pub fn exchange_event(
        &mut self,
        mut args: Vec<Value>,
        server: ServerInstance,
    ) -> (Vec<Value>, Option<u64>) {
        let tag = args
            .get(3)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if tag == "cut" {
            args[3] = json!("copy_cut");
        }
        let mut lease = None;
        if matches!(tag.as_str(), "paste" | "mix") {
            let doc = Clipboard::parse(args.get(6).and_then(Value::as_str).unwrap_or_default());
            match self
                .services
                .clipboard
                .acquire(&mut self.services.resources, doc, server)
            {
                Ok(taken) => lease = taken,
                Err(error) => {
                    args.truncate(3);
                    args.extend([json!("refused"), json!(tag), json!(error)]);
                }
            }
        }
        (args, lease)
    }

    /// **A copy published** by member `member`: the clipboard names what it
    /// is of. Answers the document as published.
    pub(crate) fn exchange_publish(
        &mut self,
        member: MemberId,
        document: &str,
        server: ServerInstance,
    ) -> Result<String, String> {
        let doc = serde_json::from_str::<Clipboard>(document)
            .map_err(|_| "invalid clipboard document".to_string())?;
        self.services
            .clipboard
            .publish(&mut self.services.resources, doc, server, member)
    }

    /// A gesture's clipboard lease let go.
    pub fn exchange_release(&mut self, lease: u64) {
        self.services
            .clipboard
            .release(&mut self.services.resources, lease);
    }
}
