use mcpeval::errtemplate::skeleton;
use mcpeval::fingerprint::{template_id, Salt};

#[test]
fn same_defect_different_values_fingerprints_the_same() {
    let salt = Salt::for_tests();
    let a = template_id(
        &salt,
        "session 0be9b59c-af70-47b0-9169-d9de92330600 died after 5 actions",
    );
    let b = template_id(
        &salt,
        "session f5a8fb32-922f-4f72-b09a-474045fd0094 died after 12 actions",
    );
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
    let id = template_id(
        &Salt::for_tests(),
        "Cannot upload /Users/someone/private.pdf",
    );
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
    assert_eq!(
        skeleton("ws://127.0.0.1:9222/session unreachable"),
        "l unreachable"
    );
    assert_eq!(
        skeleton("descriptor at /Users/a/b.json missing"),
        "descriptor at p missing"
    );
}

#[test]
fn skeleton_does_not_collapse_a_slash_inside_an_ordinary_word() {
    // A namespaced JSON-RPC method like "tools/call" must not skeletonize
    // to the same thing as an absolute path: the slash here has no token
    // boundary before it, so it is not a path.
    assert_ne!(
        skeleton("tools/call failed"),
        skeleton("tools/list failed"),
        "distinct methods must not collapse to the same template"
    );
    assert_eq!(skeleton("tools/call failed"), "tools/call failed");
    assert_eq!(skeleton("tools/list failed"), "tools/list failed");
}

#[test]
fn skeleton_still_collapses_an_absolute_path_at_a_real_token_boundary() {
    assert_eq!(skeleton("/Users/a/b missing"), "p missing");
    assert_eq!(skeleton("failed (/Users/a/b"), "failed (p");
}

#[test]
fn skeleton_does_not_collapse_the_span_between_two_contractions() {
    // Both apostrophes here are mid-word (contractions), not opening quotes,
    // so the single-quote arm must not treat the first as an opening quote
    // and swallow everything up to the second.
    assert_eq!(
        skeleton("can't connect, won't retry"),
        "can't connect, won't retry"
    );
}

#[test]
fn skeleton_still_collapses_a_properly_bounded_single_quoted_run() {
    assert_eq!(
        skeleton("saw 'unexpected token' in response"),
        "saw q in response"
    );
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

#[test]
fn salt_lives_outside_the_shareable_store_directory() {
    let dir = std::env::temp_dir().join(format!("mcpeval-saltpath-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("store")).unwrap();
    Salt::load(&dir).unwrap();
    assert!(
        dir.join(".salt").is_file(),
        "salt must live at <root>/.salt"
    );
    assert!(
        !dir.join("salt").exists(),
        "the old non-dotfile salt path must not be used"
    );
    assert!(
        !dir.join("store").join("salt").exists(),
        "the salt must never land inside the shareable store/ directory"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn concurrent_first_run_loads_all_agree_with_the_persisted_salt() {
    use std::sync::{Arc, Barrier};

    // Regression for the salt-creation TOCTOU: two shims started on first
    // run must not each mint their own salt and silently disagree about
    // which one is on disk. Every racer's returned salt must match what
    // ultimately landed on disk.
    let dir = std::env::temp_dir().join(format!("mcpeval-salt-race-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    const RACERS: usize = 16;
    let barrier = Arc::new(Barrier::new(RACERS));

    let handles: Vec<_> = (0..RACERS)
        .map(|_| {
            let dir = dir.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                Salt::load(&dir).unwrap()
            })
        })
        .collect();
    let salts: Vec<Salt> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let persisted = Salt::load(&dir).unwrap();
    for salt in &salts {
        assert_eq!(
            template_id(salt, "message"),
            template_id(&persisted, "message"),
            "a racing first-run load returned a salt that disagrees with what is on disk"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
