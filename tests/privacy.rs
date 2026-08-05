use mcpeval::privacy::valid_identifier;

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
