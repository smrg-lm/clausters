//! Every command the server answers is documented, and every command the
//! reference documents is answered.
//!
//! `docs/schemas.md` is the protocol's reference: it is what a client author
//! reads, and the three client packages are written from it. Nothing checked
//! it against the server. A command was easy to add -- one dispatch arm and a
//! handler -- and easy to forget to document, and the two lived in different
//! files with no way to compare them, so the reference could only be trusted
//! as far as somebody's memory.
//!
//! The dispatch table (`osc::server::commands()`) is what makes the comparison
//! possible: a `match` dispatches just as well but cannot be walked, so the
//! command set had no runtime existence to compare anything to.
//!
//! What this cannot check is whether the *arguments* a page describes are the
//! ones the handler reads. That is the deeper drift and it needs types the
//! protocol does not have; this catches the coarse one, which is the one that
//! has actually happened.

use std::collections::BTreeSet;

/// Addresses that are not commands: a client never sends them, so the command
/// table has no row for them and their absence is not a gap.
const NOT_COMMANDS: &[&str] = &[
    // Replies and acknowledgements.
    "/done",
    "/fail",
    // Notifications: nobody asked for these, the server sends them.
    "/node_start",
    "/node_end",
    "/node_trigger",
    "/node_move",
    "/node_off",
    "/node_on",
    // `SendReply`'s default outgoing address: the server sends it, at an
    // address the def chooses, and never receives it.
    "/reply",
];

/// Whether a backticked `/...` is one of *our* command addresses, as opposed
/// to the three other things the reference spells that way.
fn is_command_address(word: &str) -> bool {
    let Some(name) = word.strip_prefix('/') else {
        return false;
    };
    if name.is_empty() || word.contains(".reply") || NOT_COMMANDS.contains(&word) {
        return false;
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '/')
    {
        return false;
    }
    // A **group path** (`/mixer/drums`, `/1000/drums`): the node-tree address
    // syntax `/group_query` resolves, which looks like an OSC address and is
    // not one.
    if name.contains('/') {
        return false;
    }
    // An **scsynth name** cited in the mapping table (`/s_new`, `/c_getn`,
    // `/d_recv`). Its convention is a one-letter resource prefix, which is
    // exactly what clausters replaced with a spelled-out one, so the shape
    // tells them apart without a list to maintain.
    if let Some((prefix, _)) = name.split_once('_')
        && prefix.len() == 1
    {
        return false;
    }
    true
}

fn schema_text() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/schemas.md");
    std::fs::read_to_string(&path).expect("docs/schemas.md")
}

/// Every `` `/address` `` the reference mentions, replies and `.reply` suffixes
/// excluded.
fn documented() -> BTreeSet<String> {
    let text = schema_text();
    let mut out = BTreeSet::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '`' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != '`' && bytes[j] != '\n' {
                j += 1;
            }
            let word: String = bytes[start..j].iter().collect();
            // A backticked run may hold the address plus its arguments
            // (`/bus_set busIndex value`); the address is its first word.
            if let Some(first) = word.split_whitespace().next()
                && is_command_address(first)
            {
                out.insert(first.to_string());
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

fn dispatched() -> BTreeSet<String> {
    clausters::osc::server::commands()
        .iter()
        .map(|addr| addr.to_string())
        .collect()
}

#[test]
fn the_two_lists_were_actually_read() {
    // Both sides come from parsing, so a parser that stops matching would turn
    // the checks below into vacuous passes.
    let (doc, live) = (documented(), dispatched());
    assert!(
        doc.len() > 40,
        "parsed only {} addresses from the reference",
        doc.len()
    );
    assert!(
        live.len() > 40,
        "the dispatch table has only {} rows",
        live.len()
    );
    assert!(doc.contains("/synth_new") && live.contains("/synth_new"));
}

#[test]
fn every_command_the_server_answers_is_documented() {
    let doc = documented();
    let missing: Vec<_> = dispatched()
        .into_iter()
        .filter(|addr| !doc.contains(addr))
        .collect();
    assert!(
        missing.is_empty(),
        "answered by the server, absent from docs/schemas.md: {}",
        missing.join(", ")
    );
}

#[test]
fn every_documented_command_is_answered() {
    let live = dispatched();
    let phantom: Vec<_> = documented()
        .into_iter()
        .filter(|addr| !live.contains(addr) && addr.as_str() != "/server_quit")
        .collect();
    assert!(
        phantom.is_empty(),
        "documented in docs/schemas.md, answered by nothing: {}",
        phantom.join(", ")
    );
}
