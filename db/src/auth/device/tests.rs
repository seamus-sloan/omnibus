//! Unit tests for `auth::device` — covers the SQL roundtrip
//! (`register_device` / `list_devices_for_user`) plus the input-validation
//! guards on `device_name` / `client_version` (length cap, control-char
//! rejection). Validation tests are pure unit; register/list and pre-insert
//! rejection tests share an in-memory pool via `auth::test_support::pool`.

use super::*;
use crate::auth::test_support::pool;
use crate::auth::users::create_user;
use sqlx::Row;

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

/// Guard against a covering-index regression on `list_devices_for_user`.
/// Without `idx_devices_user_last_seen` the planner filters via
/// `idx_devices_user` and sorts the matched rows in memory — SQLite
/// calls this out as `USE TEMP B-TREE FOR ORDER BY`. We assert the plan
/// mentions the covering index by name and does not mention the temp
/// b-tree — a structural check that survives point-release wording
/// changes in the plan strings.
#[tokio::test]
async fn list_devices_for_user_query_plan_uses_covering_index() {
    let p = pool().await;
    // Seed two users so the planner's stats reflect real selectivity.
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    crate::auth::users::set_registration_enabled(&p, true).await.unwrap();
    let bob = create_user(&p, "bob", "hunter2-real-long").await.unwrap();
    for uid in [alice.id, bob.id] {
        for i in 0..50 {
            register_device(&p, uid, &format!("d{i}"), "ios", None)
                .await
                .unwrap();
        }
    }
    sqlx::query("ANALYZE").execute(&p).await.unwrap();

    let rows = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT id, user_id, name, client_kind, client_version, created_at, last_seen_at
           FROM devices WHERE user_id = ? ORDER BY last_seen_at DESC",
    )
    .bind(alice.id)
    .fetch_all(&p)
    .await
    .unwrap();
    let plan: String = rows
        .iter()
        .map(|r| r.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("idx_devices_user_last_seen"),
        "expected covering index in plan, got:\n{plan}",
    );
    assert!(
        !plan.contains("USE TEMP B-TREE FOR ORDER BY"),
        "expected index-only sort — plan still uses a temp b-tree:\n{plan}",
    );
}
