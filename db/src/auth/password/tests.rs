use super::*;

#[test]
fn password_roundtrips() {
    let phc = hash_password("correct horse battery staple").unwrap();
    assert!(phc.starts_with("$argon2id$"));
    assert!(verify_password("correct horse battery staple", &phc).unwrap());
    assert!(!verify_password("wrong password entirely", &phc).unwrap());
}

#[test]
fn password_policy_rejects_short() {
    let err = validate_password("short").unwrap_err();
    assert!(
        matches!(&err, AuthError::Validation(m) if m.contains("password too short")),
        "expected validation `password too short …`, got {err:?}",
    );
}

#[test]
fn password_policy_rejects_common() {
    let err = validate_password("password123").unwrap_err();
    assert!(
        matches!(&err, AuthError::Validation(m) if m == "password is too common"),
        "expected validation `password is too common`, got {err:?}",
    );
}

#[test]
fn password_policy_accepts_reasonable() {
    assert!(validate_password("xk7-banana-frog-42").is_ok());
}

/// Assert the validator rejects `input` with a `Validation` whose message
/// contains `needle` — keeps test failures debuggable by surfacing the
/// actual message when the substring drifts.
fn assert_validation_contains(input: &str, needle: &str) {
    let err = validate_username(input).unwrap_err();
    assert!(
        matches!(&err, AuthError::Validation(m) if m.contains(needle)),
        "input {input:?}: expected Validation containing {needle:?}, got {err:?}",
    );
}

#[test]
fn username_policy_rejects_empty() {
    assert_validation_contains("", "username must not be empty");
}

#[test]
fn username_policy_accepts_single_char() {
    assert!(validate_username("a").is_ok());
}

#[test]
fn username_policy_accepts_max_length() {
    let name: String = "a".repeat(MAX_USERNAME_LEN);
    assert!(validate_username(&name).is_ok());
}

#[test]
fn username_policy_rejects_over_max_length() {
    let name: String = "a".repeat(MAX_USERNAME_LEN + 1);
    assert_validation_contains(&name, "username too long");
}

#[test]
fn username_policy_rejects_leading_whitespace() {
    assert_validation_contains(" alice", "leading or trailing whitespace");
}

#[test]
fn username_policy_rejects_trailing_whitespace() {
    assert_validation_contains("alice ", "leading or trailing whitespace");
}

#[test]
fn username_policy_rejects_only_whitespace() {
    // All-whitespace input has trim() != self, so it surfaces as the
    // whitespace message rather than the empty message — either is a
    // reject, but lock the wording to keep callers' error UX stable.
    assert_validation_contains("   ", "leading or trailing whitespace");
}

#[test]
fn username_policy_rejects_embedded_tab() {
    assert_validation_contains("ali\tce", "invalid control character");
}

#[test]
fn username_policy_rejects_embedded_newline() {
    assert_validation_contains("ali\nce", "invalid control character");
}

#[test]
fn username_policy_rejects_embedded_null() {
    assert_validation_contains("ali\0ce", "invalid control character");
}

#[test]
fn username_policy_rejects_low_control_char() {
    assert_validation_contains("ali\x1fce", "invalid control character");
}

#[test]
fn username_policy_rejects_delete_char() {
    assert_validation_contains("ali\x7fce", "invalid control character");
}

#[test]
fn username_policy_accepts_reasonable() {
    assert!(validate_username("alice").is_ok());
    assert!(validate_username("Alice.Smith-42_").is_ok());
    assert!(validate_username("user@example.com").is_ok());
}
