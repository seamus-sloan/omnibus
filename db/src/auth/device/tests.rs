//! Unit tests for `auth::device` — covers the SQL roundtrip
//! (`register_device` / `list_devices_for_user`) plus the input-validation
//! guards on `device_name` / `client_version` (length cap, control-char
//! rejection). Validation tests are pure unit; register/list and pre-insert
//! rejection tests share an in-memory pool via `auth::test_support::pool`.

use super::*;
use crate::auth::session::create_session;
use crate::auth::test_support::pool;
use crate::auth::users::create_user;
use crate::auth::SessionKind;

#[tokio::test]
async fn device_register_and_list() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let d = register_device(&p, u.id, "Phone", "ios", Some("1.0.0"))
        .await
        .unwrap();
    let list = list_devices_for_user(&p, u.id).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, d.id);
    assert_eq!(list[0].client_kind, "ios");
}

#[test]
fn validate_device_name_accepts_short_unicode_label() {
    // Char count, not byte count: a 4-byte emoji is one character.
    let s = "📱 Phone — Alice";
    assert!(validate_device_name(Some(s)).is_ok());
    assert!(validate_device_name(None).is_ok());
}

#[test]
fn validate_device_name_rejects_overlength() {
    let too_long: String = "a".repeat(MAX_DEVICE_NAME_CHARS + 1);
    let err = validate_device_name(Some(&too_long)).unwrap_err();
    match err {
        AuthError::Validation(msg) => assert!(
            msg.contains("invalid device_name") && msg.contains("too long"),
            "expected Validation about device_name being too long, got {msg:?}",
        ),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn validate_device_name_at_exact_cap_passes() {
    let exact: String = "a".repeat(MAX_DEVICE_NAME_CHARS);
    assert!(validate_device_name(Some(&exact)).is_ok());
}

#[test]
fn validate_device_name_rejects_control_characters() {
    // CR/LF guard the log-injection vector — without it a `device_name`
    // of `"benign\nFAKE LOG LINE"` would split a tracing event into two.
    for raw in ["benign\nlog", "tab\there", "null\0byte", "bell\x07"] {
        let err = validate_device_name(Some(raw)).unwrap_err();
        assert!(
            matches!(&err, AuthError::Validation(m)
                if m.contains("invalid device_name") && m.contains("control characters")),
            "expected control-char rejection for {raw:?}, got {err:?}",
        );
    }
}

#[test]
fn validate_client_version_caps_at_64_chars() {
    assert!(validate_client_version(Some("1.2.3-rc.10+build.abc")).is_ok());
    let too_long: String = "1".repeat(MAX_CLIENT_VERSION_CHARS + 1);
    let err = validate_client_version(Some(&too_long)).unwrap_err();
    assert!(
        matches!(&err, AuthError::Validation(m)
            if m.contains("invalid client_version") && m.contains("too long")),
        "expected Validation about client_version being too long, got {err:?}",
    );
}

#[tokio::test]
async fn register_device_rejects_overlong_name_before_insert() {
    // No row is inserted on rejection — the validator runs before the
    // SQL roundtrip, so list_devices_for_user stays empty.
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let too_long: String = "x".repeat(MAX_DEVICE_NAME_CHARS + 1);
    let err = register_device(&p, u.id, &too_long, "ios", None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AuthError::Validation(m) if m.contains("invalid device_name")),
        "expected Validation about device_name, got {err:?}",
    );
    let list = list_devices_for_user(&p, u.id).await.unwrap();
    assert!(list.is_empty(), "rejection must not leave a partial row");
}

#[tokio::test]
async fn register_device_rejects_control_char_in_client_version() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let err = register_device(&p, u.id, "Phone", "ios", Some("1.0\n0"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AuthError::Validation(m)
            if m.contains("invalid client_version") && m.contains("control characters")),
        "expected Validation about client_version control chars, got {err:?}",
    );
}

#[tokio::test]
async fn failed_session_insert_after_register_device_rolls_back_device_row() {
    // Regression for #627: `issue_session` wraps `register_device` +
    // `create_session` in a single transaction so a failed session insert
    // can't leave an orphan device row. Force `create_session` to fail
    // inside the shared transaction by pointing it at a non-existent
    // `device_id` (violates the `sessions.device_id -> devices.id` FK,
    // enforced by the `PRAGMA foreign_keys = ON` in `init_db`), then drop
    // the tx and assert the successfully-inserted device row is gone.
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let bogus_device_id = 999_999_999;
    {
        let mut tx = p.begin().await.unwrap();
        let device = register_device(&mut *tx, u.id, "Phone", "ios", Some("1.0.0"))
            .await
            .expect("device insert should succeed inside the transaction");
        assert_ne!(device.id, bogus_device_id, "test sentinel collided with real id");

        let session_err = create_session(
            &mut *tx,
            u.id,
            Some(bogus_device_id),
            SessionKind::Cookie,
            3_600,
        )
        .await
        .expect_err("create_session must reject the bogus device_id FK");
        assert!(
            matches!(session_err, AuthError::Internal(_)),
            "expected FK violation surfaced as AuthError::Internal, got {session_err:?}",
        );
        // Drop `tx` without committing — mirrors `persist_session`'s
        // early return on any error in the compound insert path.
    }

    let list = list_devices_for_user(&p, u.id).await.unwrap();
    assert!(
        list.is_empty(),
        "device row must not persist when the compound session insert fails, got {list:?}",
    );
}
