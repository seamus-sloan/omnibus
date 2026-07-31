//! Tests for `classify_register_error`: routing a server error string to
//! the matching `RegisterError` variant. Password-scoring coverage lives
//! with the shared helper in `components::auth::strength`; these tests
//! cover only register-specific error routing.

use super::*;

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
