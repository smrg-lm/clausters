#[test]
fn the_routing_table_names_screen_state_and_nothing_a_domain_reads() {
    for tag in super::NOT_AN_EDIT {
        assert!(super::is_screen_state(tag), "{tag}");
    }
    for tag in ["clips", "tracks", "notes", "points", "samples", "level"] {
        assert!(!super::is_screen_state(tag), "{tag} is an edit");
    }
}

#[test]
fn the_routing_table_says_each_tag_once() {
    let mut seen = super::NOT_AN_EDIT.to_vec();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(seen.len(), before);
}
