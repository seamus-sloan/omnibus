//! Unit tests for `auth::device`: the `register_device` /
//! `list_devices_for_user` SQL roundtrip, the `device_name` / `client_version`
//! validation guards, the `LIST_DEVICES_LIMIT` response ceiling, and the
//! `trg_devices_cap_per_user` eviction trigger.

use std::collections::HashSet;

use sqlx::{Row, SqlitePool};

use super::*;
use crate::auth::session::create_session;
use crate::auth::test_support::pool;
use crate::auth::users::create_user;
use crate::auth::SessionKind;

/// Bulk-insert `count` device rows for `user_id` without going through
/// `register_device` — used only to exercise `list_devices_for_user`'s
/// `LIST_DEVICES_LIMIT` in isolation, above what the per-user registration
/// cap would ever let `register_device` accumulate.
async fn seed_devices_raw(pool: &SqlitePool, user_id: i64, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO devices (user_id, name, client_kind, created_at, last_seen_at)
        SELECT ?, 'd' || i, 'ios', i, i FROM n
        ",
    )
    .bind(count)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

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

/// `list_devices_for_user` must not return more than
/// `LIST_DEVICES_LIMIT` rows even when the underlying table holds more.
/// `trg_devices_cap_per_user` would otherwise mask this by keeping the
/// table itself under the limit, so the trigger is dropped here to
/// exercise the SELECT-side ceiling in isolation.
#[tokio::test]
async fn list_devices_for_user_caps_response_at_list_devices_limit() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    sqlx::query("DROP TRIGGER trg_devices_cap_per_user")
        .execute(&p)
        .await
        .unwrap();
    let over_cap = LIST_DEVICES_LIMIT + 50;
    seed_devices_raw(&p, u.id, over_cap).await;

    let list = list_devices_for_user(&p, u.id).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_DEVICES_LIMIT,
        "list_devices_for_user must not return more than LIST_DEVICES_LIMIT rows",
    );
}

/// `register_device` must not let a single user
/// accumulate unbounded devices — `trg_devices_cap_per_user` evicts the
/// least-recently-seen device(s) beyond `MAX_DEVICES_PER_USER` on every
/// INSERT, keeping the most-recently-registered devices.
#[tokio::test]
async fn register_device_evicts_oldest_when_over_max_devices_per_user() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let total = MAX_DEVICES_PER_USER + 5;
    for i in 0..total {
        register_device(&p, u.id, &format!("d{i}"), "ios", None)
            .await
            .unwrap();
    }

    let list = list_devices_for_user(&p, u.id).await.unwrap();
    assert_eq!(
        list.len() as i64,
        MAX_DEVICES_PER_USER,
        "register_device must evict down to MAX_DEVICES_PER_USER devices",
    );
    let names: HashSet<&str> = list.iter().map(|d| d.name.as_str()).collect();
    for i in 0..5 {
        let evicted = format!("d{i}");
        assert!(
            !names.contains(evicted.as_str()),
            "earliest-registered device {evicted} should have been evicted, got {names:?}",
        );
    }
    for i in 5..total {
        let kept = format!("d{i}");
        assert!(
            names.contains(kept.as_str()),
            "most-recently-registered device {kept} should be retained, got {names:?}",
        );
    }
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
    crate::auth::users::set_registration_enabled(&p, true)
        .await
        .unwrap();
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

#[tokio::test]
async fn failed_session_insert_after_register_device_rolls_back_device_row() {
    // `issue_session` wraps `register_device` +
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
        assert_ne!(
            device.id, bogus_device_id,
            "test sentinel collided with real id"
        );

        // `NewSession` doesn't implement `Debug`, so `.expect_err` won't
        // compile — pattern-match the `Err` explicitly instead.
        let Err(session_err) = create_session(
            &mut *tx,
            u.id,
            Some(bogus_device_id),
            SessionKind::Cookie,
            3_600,
        )
        .await
        else {
            panic!("create_session must reject the bogus device_id FK");
        };
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

#[tokio::test]
async fn revoke_device_removes_the_row() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let d = register_device(&p, u.id, "Phone", "ios", None)
        .await
        .unwrap();
    revoke_device(&p, d.id).await.unwrap();
    let list = list_devices_for_user(&p, u.id).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn revoke_device_returns_device_not_found_for_unknown_id() {
    let p = pool().await;
    let err = revoke_device(&p, 999_999).await.unwrap_err();
    assert!(matches!(err, AuthError::DeviceNotFound));
}

#[tokio::test]
async fn revoke_device_returns_internal_when_pool_closed() {
    let p = pool().await;
    p.close().await;
    let err = revoke_device(&p, 1).await.unwrap_err();
    assert!(matches!(err, AuthError::Internal(_)));
}

#[tokio::test]
async fn revoke_device_also_revokes_its_live_sessions() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let d = register_device(&p, u.id, "Phone", "ios", None)
        .await
        .unwrap();
    let ns = create_session(&p, u.id, Some(d.id), SessionKind::Bearer, 3600)
        .await
        .unwrap();

    revoke_device(&p, d.id).await.unwrap();

    let err = crate::auth::lookup_session(&p, &ns.raw_token)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AuthError::SessionNotFound),
        "revoking a device must also revoke sessions still tied to it"
    );
}
