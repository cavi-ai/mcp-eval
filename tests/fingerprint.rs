use mcpeval::errtemplate::skeleton;
use mcpeval::fingerprint::{template_id, Salt};

#[test]
fn same_defect_different_values_fingerprints_the_same() {
    let salt = Salt::for_tests();
    let a = template_id(&salt, "session 0be9b59c-af70-47b0-9169-d9de92330600 died after 5 actions");
    let b = template_id(&salt, "session f5a8fb32-922f-4f72-b09a-474045fd0094 died after 12 actions");
    assert_eq!(a, b);
}

#[test]
fn different_defects_fingerprint_differently() {
    let salt = Salt::for_tests();
    let a = template_id(&salt, "failed to bind companion server");
    let b = template_id(&salt, "journal line 6 is corrupt");
    assert_ne!(a, b);
}

#[test]
fn a_different_salt_changes_the_fingerprint() {
    let mine = template_id(&Salt::for_tests(), "boom");
    let other = template_id(&Salt::from_bytes([7u8; 32]), "boom");
    assert_ne!(mine, other);
}

#[test]
fn the_fingerprint_does_not_contain_the_message() {
    let id = template_id(&Salt::for_tests(), "Cannot upload /Users/someone/private.pdf");
    assert_eq!(id.len(), 16);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!id.contains("private"));
}

#[test]
fn skeleton_collapses_values_but_keeps_structure() {
    assert_eq!(
        skeleton("Session 0BE9B59C-AF70-47B0-9169-D9DE92330600 died after 5 actions"),
        "session u died after 0 actions"
    );
    assert_eq!(skeleton("cannot open \"/tmp/x\""), "cannot open q");
    assert_eq!(skeleton("ws://127.0.0.1:9222/session unreachable"), "l unreachable");
    assert_eq!(skeleton("descriptor at /Users/a/b.json missing"), "descriptor at p missing");
}

#[test]
fn salt_persists_across_loads() {
    let dir = std::env::temp_dir().join(format!("mcpeval-salt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let first = Salt::load(&dir).unwrap();
    let second = Salt::load(&dir).unwrap();
    assert_eq!(
        template_id(&first, "same message"),
        template_id(&second, "same message")
    );
}
