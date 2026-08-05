use mcpeval::privacy::{valid_identifier, valid_server, valid_tool};

#[test]
fn server_labels_reject_embedded_nul_delimiters() {
    assert!(!valid_server("alpha\0beta"));
}

#[test]
fn empty_string_is_rejected() {
    assert!(!valid_identifier(""));
}

#[test]
fn a_leading_digit_is_rejected() {
    assert!(!valid_identifier("1browserCommandFailed"));
}

#[test]
fn sixty_four_bytes_is_accepted() {
    let value = format!("a{}", "b".repeat(63));
    assert_eq!(value.len(), 64);
    assert!(valid_identifier(&value));
}

#[test]
fn sixty_five_bytes_is_rejected() {
    let value = format!("a{}", "b".repeat(64));
    assert_eq!(value.len(), 65);
    assert!(!valid_identifier(&value));
}

#[test]
fn a_space_is_rejected() {
    assert!(!valid_identifier("browser command failed"));
}

// `valid_tool` is the actual gate `correlate::Correlator` uses (and
// `Store::append` re-validates with), not `valid_identifier` — see I6 in
// the final-review-findings fix wave. It is a different, wider grammar:
// non-empty, at most 128 bytes, ASCII alphanumeric plus `_`, `-`, `.`, `:`,
// with no leading-letter requirement.

#[test]
fn valid_tool_accepts_128_bytes() {
    let value = "a".repeat(128);
    assert_eq!(value.len(), 128);
    assert!(valid_tool(&value));
}

#[test]
fn valid_tool_rejects_129_bytes() {
    let value = "a".repeat(129);
    assert_eq!(value.len(), 129);
    assert!(!valid_tool(&value));
}

#[test]
fn valid_tool_accepts_a_leading_digit() {
    // Unlike `valid_identifier`, `valid_tool` has no leading-letter rule.
    assert!(valid_tool("1browserCommandFailed"));
}

#[test]
fn valid_tool_rejects_punctuation_outside_its_grammar() {
    assert!(!valid_tool("!!!"));
}
