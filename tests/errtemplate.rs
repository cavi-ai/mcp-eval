use mcpeval::errtemplate::normalize;

const TEMPLATE: &str = "{message}";

#[test]
fn sensitive_values_and_human_text_never_survive_normalization() {
    let cases = [
        "session 0be9b59c-af70-47b0-9169-d9de92330600 not found",
        "journal line 958 is corrupt",
        "Cannot upload \"/Users/someone/private.pdf\": not shared",
        "descriptor at /Users/someone/Library/x.json missing",
        "ws://127.0.0.1:9222/session unreachable",
        "extension observation failed: the content action failed",
        "canary-secret-8f2d4a1c-keep-private",
    ];

    for message in cases {
        let normalized = normalize(message);
        assert_eq!(normalized, TEMPLATE);
        assert!(!normalized.contains(message));
    }
}

#[test]
fn different_messages_normalize_identically() {
    assert_eq!(
        normalize("ordinary human explanation"),
        normalize("a different canary-secret-8f2d4a1c message")
    );
}
