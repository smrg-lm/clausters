//! Monospaced fonts installed on the system (WebKitGTK does not expose this list).

use std::collections::BTreeSet;

/// Family names, sorted and deduplicated. It is `async` so that scanning the
/// font folders does not block the main thread.
#[tauri::command]
pub async fn list_mono_fonts() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db.faces()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        // Emoji fonts declare themselves fixed-width but are no use for writing code.
        .filter(|name| !name.to_lowercase().contains("emoji"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
