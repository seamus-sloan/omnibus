use super::*;

#[test]
fn score_password_empty_is_zero() {
    let (score, label, rules) = score_password("");
    assert_eq!(score.value(), 0);
    assert_eq!(label, "empty");
    assert_eq!(rules, [false, false, false]);
}

#[test]
fn score_password_grows_with_length_and_classes() {
    let (s, _, _) = score_password("abcd");
    assert_eq!(s.value(), 1);
    let (s, _, _) = score_password("AbCdEfGh");
    assert_eq!(s.value(), 3);
    let (s, label, rules) = score_password("AbCdEfGh1!2x");
    assert_eq!(s.value(), 4);
    assert_eq!(label, "strong");
    assert_eq!(rules, [true, true, true]);
}

#[test]
fn score_password_rules_track_thresholds() {
    let (_, _, rules) = score_password("Ab1");
    assert_eq!(rules, [false, true, true]);
    let (_, _, rules) = score_password("abcdefghijk1");
    assert_eq!(rules, [true, false, true]);
}

#[test]
fn score_password_length_rule_boundary() {
    // Right at the server-policy boundary (10 chars). 9-char inputs
    // must report length-not-met; 10-char inputs must report met.
    let (_, _, rules) = score_password("abcdefgh1");
    assert!(!rules[0], "9-char input must not satisfy len_ok");
    let (_, _, rules) = score_password("abcdefghi1");
    assert!(rules[0], "10-char input must satisfy len_ok");
    let (_, _, rules) = score_password("abcdefghij1");
    assert!(rules[0], "11-char input must satisfy len_ok");
}

#[test]
fn score_password_label_distinguishes_empty_from_short() {
    // Empty -> "empty"; any non-empty input -> at least "weak"
    // (covers the 1–3 char range where score=0 but typed content
    // exists, so the meter shouldn't lie about being empty).
    let (_, label, _) = score_password("");
    assert_eq!(label, "empty");
    let (_, label, _) = score_password("a");
    assert_eq!(label, "weak");
    let (_, label, _) = score_password("ab");
    assert_eq!(label, "weak");
}

#[test]
fn classify_register_error_routes_username() {
    match classify_register_error("username already exists") {
        RegisterError::Username(m) => assert_eq!(m, "username already exists"),
        other => unreachable!("expected Username variant, got {other:?}"),
    }
}

#[test]
fn classify_register_error_routes_password() {
    match classify_register_error("password is too short") {
        RegisterError::Password(m) => assert_eq!(m, "password is too short"),
        other => unreachable!("expected Password variant, got {other:?}"),
    }
}

#[test]
fn classify_register_error_falls_back_to_other() {
    match classify_register_error("500: internal server error") {
        RegisterError::Other(m) => assert_eq!(m, "500: internal server error"),
        other => unreachable!("expected Other variant, got {other:?}"),
    }
}

// `data_error_message` (in the parent `auth` module) must keep the HTTP
// body so this classifier can still route field errors on the mobile
// path. Without the body, `DataError`'s own Display ("server returned
// 400") would strand every server diagnostic in the `Other` bucket.
#[cfg(feature = "mobile")]
#[test]
fn data_error_message_preserves_http_body_for_classification() {
    let msg = super::super::data_error_message(crate::data::DataError::Http {
        status: 409,
        body: "username already exists".into(),
    });
    assert_eq!(msg, "409: username already exists");
    assert!(matches!(
        classify_register_error(&msg),
        RegisterError::Username(_)
    ));
}

#[cfg(feature = "mobile")]
#[test]
fn data_error_message_falls_back_to_display_without_body() {
    assert_eq!(
        super::super::data_error_message(crate::data::DataError::Unauthorized),
        "unauthorized"
    );
    assert_eq!(
        super::super::data_error_message(crate::data::DataError::Http {
            status: 500,
            body: String::new(),
        }),
        "server returned 500"
    );
}
