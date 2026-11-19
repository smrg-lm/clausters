//! **A named GuiDef as it is kept**, whichever store keeps it: the record a
//! file holds (`{"id": <i32>, "gui": <tree>}`) and the file's name -- one
//! format for the disk's store (`native::store`) and the page's
//! (`web::store`), so a GuiDef saved by either reads in both.

use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the GuiDefs are kept, under a data directory -- beside the
/// SynthDefs and the GraphDefs, as the server keeps them.
pub const GUIDEFS: &str = "defs/guidefs";

/// A persisted GuiDef record: the id it was defined with plus its tree JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGuiDef {
    pub id: i32,
    pub gui: Value,
}

/// GuiDef `id`'s tree as the record a store writes.
pub fn record(id: i32, tree_json: &[u8]) -> io::Result<Vec<u8>> {
    let gui: Value = serde_json::from_slice(tree_json).map_err(io::Error::other)?;
    serde_json::to_vec_pretty(&SavedGuiDef { id, gui }).map_err(io::Error::other)
}

/// A record read back: the id and the tree JSON, ready to replay as a
/// `/gui_def`.
pub fn read_record(bytes: &[u8]) -> io::Result<(i32, Vec<u8>)> {
    let record: SavedGuiDef = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let tree = serde_json::to_vec(&record.gui).map_err(io::Error::other)?;
    Ok((record.id, tree))
}

/// The file a GuiDef named `name` is kept in, under [`GUIDEFS`].
pub fn file_name(name: &str) -> String {
    format!("{}.json", sanitize_name(name))
}

/// Maps an arbitrary def name to a safe file stem (percent-encoding anything
/// outside `[A-Za-z0-9._-]`), matching the server's `defstore::sanitize_name`.
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record reads back as what was kept, and a name is a safe file.
    #[test]
    fn a_record_reads_back() {
        let bytes = record(7, br#"{"type":"window"}"#).unwrap();
        let (id, tree) = read_record(&bytes).unwrap();
        assert_eq!(id, 7);
        assert_eq!(tree, br#"{"type":"window"}"#);
        assert_eq!(file_name("a b/c"), "a%20b%2Fc.json");
    }
}
