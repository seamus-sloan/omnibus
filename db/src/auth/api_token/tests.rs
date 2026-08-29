//! Tests for the API-token lifecycle: create/lookup round-tripping with the
//! `omni_` prefix, at-rest hashing, revocation, listing, name validation,
//! no-idle-expiry behaviour, and the `resolve_token` prefix routing.

use omnibus_shared::UserPermissions;

use super::*;
use crate::auth::session::resolve_token;
use crate::auth::test_support::pool;
use crate::auth::users::{admin_create_user, create_user};
use crate::auth::SessionKind;

/// Self-registration closes after the first user, so second accounts are
/// minted through the admin path.
async fn second_user(p: &sqlx::SqlitePool, name: &str) -> i64 {
    admin_create_user(
        p,
        name,
        "hunter2-real-long",
        UserPermissions {
            is_admin: false,
            can_upload: false,
            can_edit: false,
            can_download: true,
        },
    )
    .await
    .unwrap()
    .id
}

#[tokio::test]
async fn create_api_token_and_lookup_api_token_round_trip() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let nt = create_api_token(&p, u.id, "mcp laptop").await.unwrap();
    assert!(nt.raw_token.starts_with(API_TOKEN_PREFIX));
    // Pins the minted length `is_api_token_shaped` routes on — a change to
    // `generate_token`'s output length must fail here, not misroute auth.
    assert_eq!(nt.raw_token.len(), API_TOKEN_RAW_LEN);
    assert_eq!(nt.token.name, "mcp laptop");
    assert!(nt.token.last_used_at.is_none());

    let (user2, tok2) = lookup_api_token(&p, &nt.raw_token).await.unwrap();
    assert_eq!(user2.id, u.id);
    assert_eq!(tok2.id, nt.token.id);
}

#[tokio::test]
async fn api_token_lookup_hashes_token_at_rest() {
    // Proves the db does not store the raw token: no row's hash column may
    // equal the raw token bytes.
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let nt = create_api_token(&p, u.id, "mcp").await.unwrap();
    let raw_as_hash: Option<i64> =
        sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash = ?")
            .bind(nt.raw_token.as_bytes())
            .fetch_optional(&p)
            .await
            .unwrap();
    assert!(raw_as_hash.is_none(), "raw token must not be stored");
}

#[tokio::test]
async fn api_token_survives_a_stale_last_used_at() {
    // AC1: unlike sessions, a token untouched for far longer than the
    // session idle timeout still authenticates.
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let nt = create_api_token(&p, u.id, "mcp").await.unwrap();
    let year_ago = crate::auth::now_unix() - 365 * 24 * 60 * 60;
    sqlx::query("UPDATE api_tokens SET created_at = ?, last_used_at = ? WHERE id = ?")
        .bind(year_ago)
        .bind(year_ago)
        .bind(nt.token.id)
        .execute(&p)
        .await
        .unwrap();
    let (_, tok) = lookup_api_token(&p, &nt.raw_token).await.unwrap();
    // The lookup also proves the opportunistic touch: a year-old
    // `last_used_at` is past the threshold, so it was just rewritten.
    assert!(tok.id == nt.token.id);
    let touched: i64 = sqlx::query_scalar("SELECT last_used_at FROM api_tokens WHERE id = ?")
        .bind(nt.token.id)
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(touched > year_ago, "lookup should touch last_used_at");
}

#[tokio::test]
async fn revoked_api_token_is_rejected() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let nt = create_api_token(&p, u.id, "mcp").await.unwrap();
    revoke_api_token_for_user(&p, u.id, nt.token.id)
        .await
        .unwrap();
    let err = lookup_api_token(&p, &nt.raw_token).await.unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn unknown_api_token_is_rejected() {
    let p = pool().await;
    let err = lookup_api_token(&p, "omni_definitely-not-a-token")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn create_api_token_rejects_blank_and_oversized_names() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    // `.err().unwrap()` rather than `.unwrap_err()` — NewApiToken has no
    // Debug (deliberately, it carries the secret), which unwrap_err needs.
    let err = create_api_token(&p, u.id, "   ").await.err().unwrap();
    assert!(matches!(err, AuthError::Validation(_)));
    let long = "x".repeat(MAX_API_TOKEN_NAME_CHARS + 1);
    let err = create_api_token(&p, u.id, &long).await.err().unwrap();
    assert!(matches!(err, AuthError::Validation(_)));
}

#[tokio::test]
async fn list_api_tokens_for_user_omits_revoked_and_other_users() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let other_id = second_user(&p, "bob").await;
    let keep = create_api_token(&p, u.id, "keep").await.unwrap();
    let gone = create_api_token(&p, u.id, "gone").await.unwrap();
    create_api_token(&p, other_id, "bobs").await.unwrap();
    revoke_api_token_for_user(&p, u.id, gone.token.id)
        .await
        .unwrap();

    let listed = list_api_tokens_for_user(&p, u.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, keep.token.id);
}

#[tokio::test]
async fn revoke_api_token_for_user_rejects_unknown_and_foreign_ids() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let other_id = second_user(&p, "bob").await;
    let nt = create_api_token(&p, other_id, "bobs").await.unwrap();

    // Unknown id.
    let err = revoke_api_token_for_user(&p, u.id, 999_999)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ApiTokenNotFound));
    // Someone else's token.
    let err = revoke_api_token_for_user(&p, u.id, nt.token.id)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ApiTokenNotFound));
    // Already revoked.
    revoke_api_token_for_user(&p, other_id, nt.token.id)
        .await
        .unwrap();
    let err = revoke_api_token_for_user(&p, other_id, nt.token.id)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ApiTokenNotFound));
}

#[tokio::test]
async fn resolve_token_routes_omni_prefix_to_api_tokens() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let nt = create_api_token(&p, u.id, "mcp").await.unwrap();
    let (user, session) = resolve_token(&p, &nt.raw_token).await.unwrap();
    assert_eq!(user.id, u.id);
    assert_eq!(session.kind, SessionKind::ApiToken);
    assert_eq!(session.id, 0, "API-token principal uses the 0 sentinel");
    assert_eq!(session.expires_at, i64::MAX);
}

#[tokio::test]
async fn resolve_token_routes_an_omni_prefixed_session_token_to_sessions() {
    // Session tokens are 43-char base64url values whose alphabet includes
    // `_`, so one can (astronomically rarely) begin with `omni_`. The length
    // check in `is_api_token_shaped` must keep it on the sessions path.
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let raw = "omni_abcdefghijklmnopqrstuvwxyz0123456789AB";
    assert_eq!(raw.len(), 43, "same length as a real session token");
    let hash = crate::auth::hash_token(raw);
    let expires = crate::auth::now_unix() + 3600;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, kind, expires_at) VALUES (?, ?, 'bearer', ?)",
    )
    .bind(&hash)
    .bind(u.id)
    .bind(expires)
    .execute(&p)
    .await
    .unwrap();

    let (user, session) = resolve_token(&p, raw).await.unwrap();
    assert_eq!(user.id, u.id);
    assert_eq!(session.kind, SessionKind::Bearer);
}

#[tokio::test]
async fn resolve_token_still_resolves_plain_session_tokens() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let ns = crate::auth::create_session(&p, u.id, None, SessionKind::Bearer, 3600, None)
        .await
        .unwrap();
    let (user, session) = resolve_token(&p, &ns.raw_token).await.unwrap();
    assert_eq!(user.id, u.id);
    assert_eq!(session.id, ns.session.id);
    assert_eq!(session.kind, SessionKind::Bearer);
}
