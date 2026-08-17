//! Tests for the OPDS HTTP Basic path: the SHA-256 cache key, the
//! verified-credential cache's TTL and full-cache sweep (driven through the
//! `_at` clock seam rather than a real minute), and the `OpdsAuthUser`
//! extractor's session-first fall-through, including the branches a cache
//! hit takes when the user row moves underneath it.

use axum::{body::Body, http::Request, routing::get, Extension, Router};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use omnibus_db as db;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;

const PASSWORD: &str = "opds-basic-pass-1";

/// `Authorization: Basic` header value for `username:password`, the only
/// scheme an OPDS client like KOReader can send.
fn basic_header(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    )
}

fn get_with_headers(uri: &str, headers: &[(header::HeaderName, String)]) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value.as_str());
    }
    builder.body(Body::empty()).unwrap()
}

async fn whoami(user: OpdsAuthUser) -> String {
    user.0.username
}

async fn can_download(user: OpdsAuthUser) -> String {
    user.0.can_download.to_string()
}

/// Minimal router gated on [`OpdsAuthUser`], layered exactly like
/// `opds_router` — the pool plus the shared [`BasicAuthState`]. Returns the
/// state too so a test can assert on the cache directly.
async fn fixture() -> (Router, SqlitePool, Arc<BasicAuthState>) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let basic = BasicAuthState::new_shared();
    let app = Router::new()
        .route("/whoami", get(whoami))
        .route("/can-download", get(can_download))
        .layer(Extension(pool.clone()))
        .layer(Extension(basic.clone()));
    (app, pool, basic)
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn challenge_of(res: &axum::response::Response) -> String {
    res.headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

// ------------------------------------------------------------- Cache key

#[test]
fn cred_cache_key_folds_username_case_but_never_password_case() {
    // The username is matched `COLLATE NOCASE` by `verify_login`, so two
    // casings of one username must share a cache entry; the password must
    // not, or a near-miss would ride the cache in.
    assert_eq!(
        cred_cache_key("Reader", PASSWORD),
        cred_cache_key("reader", PASSWORD)
    );
    assert_ne!(
        cred_cache_key("reader", PASSWORD),
        cred_cache_key("reader", &PASSWORD.to_uppercase())
    );
}

#[test]
fn cred_cache_key_separates_the_fields_so_a_shifted_split_differs() {
    // Without the NUL separator, ("ab", "c") and ("a", "bc") would hash the
    // same bytes and one credential pair would authenticate the other.
    assert_ne!(cred_cache_key("ab", "c"), cred_cache_key("a", "bc"));
}

// ----------------------------------------------------------- Cache state

#[tokio::test]
async fn cached_user_id_returns_the_stored_id_within_the_ttl() {
    let state = BasicAuthState::new_shared();
    let key = cred_cache_key("reader", PASSWORD);
    let t0 = Instant::now();
    state.store_at(key, 7, t0).await;
    assert_eq!(
        state
            .cached_user_id_at(&key, t0 + VERIFIED_TTL - Duration::from_secs(1))
            .await,
        Some(7)
    );
}

#[tokio::test]
async fn cached_user_id_returns_none_for_an_unknown_key() {
    let state = BasicAuthState::new_shared();
    let key = cred_cache_key("reader", PASSWORD);
    assert_eq!(state.cached_user_id_at(&key, Instant::now()).await, None);
}

#[tokio::test]
async fn cached_user_id_expires_and_drops_an_entry_once_the_ttl_elapses() {
    let state = BasicAuthState::new_shared();
    let key = cred_cache_key("reader", PASSWORD);
    let t0 = Instant::now();
    state.store_at(key, 7, t0).await;
    assert_eq!(state.cached_user_id_at(&key, t0 + VERIFIED_TTL).await, None);
    // The stale entry is removed on the miss, not merely ignored — a
    // password change must not leave its old hash sitting in the map.
    assert!(state.verified.lock().await.is_empty());
}

#[tokio::test]
async fn cached_user_id_prunes_stale_entries_once_the_cache_is_full() {
    let state = BasicAuthState::new_shared();
    let t0 = Instant::now();
    for i in 0..MAX_VERIFIED_ENTRIES {
        state
            .store_at(
                cred_cache_key(&format!("reader-{i}"), PASSWORD),
                i as i64,
                t0,
            )
            .await;
    }
    let expired_at = t0 + VERIFIED_TTL;
    let fresh_key = cred_cache_key("fresh-reader", PASSWORD);
    state.store_at(fresh_key, 42, expired_at).await;
    assert_eq!(
        state.verified.lock().await.len(),
        MAX_VERIFIED_ENTRIES + 1,
        "sweep must be triggered by a lookup, not by the insert"
    );

    assert_eq!(
        state.cached_user_id_at(&fresh_key, expired_at).await,
        Some(42)
    );
    assert_eq!(
        state.verified.lock().await.len(),
        1,
        "every stale entry is swept, and only the fresh one survives"
    );
}

// ------------------------------------------------------------- Extractor

#[tokio::test]
async fn opds_auth_user_falls_through_to_basic_when_no_session_credentials_are_present() {
    let (app, pool, basic) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let res = app
        .oneshot(get_with_headers(
            "/whoami",
            &[(
                header::AUTHORIZATION,
                basic_header("basic-reader", PASSWORD),
            )],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "basic-reader");
    assert_eq!(
        basic.verified.lock().await.len(),
        1,
        "a successful verify must be cached so the next request skips Argon2"
    );
}

#[tokio::test]
async fn opds_auth_user_rejects_a_wrong_password_with_a_basic_challenge() {
    let (app, pool, _basic) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let res = app
        .oneshot(get_with_headers(
            "/whoami",
            &[(
                header::AUTHORIZATION,
                basic_header("basic-reader", "wrong-password"),
            )],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge_of(&res).starts_with("Basic realm="));
}

#[tokio::test]
async fn opds_auth_user_rejects_missing_or_unparsable_credentials_with_a_basic_challenge() {
    let (app, _pool, _basic) = fixture().await;
    for headers in [
        Vec::new(),
        vec![(header::AUTHORIZATION, "Basic !!!not-base64".to_string())],
        vec![(header::AUTHORIZATION, "Basic".to_string())],
    ] {
        let res = app
            .clone()
            .oneshot(get_with_headers("/whoami", &headers))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "headers={headers:?}"
        );
        assert!(challenge_of(&res).starts_with("Basic realm="));
    }
}

#[tokio::test]
async fn opds_auth_user_prefers_a_session_cookie_over_basic_credentials() {
    // Fall-through ordering: the session path runs first, so a request
    // carrying both resolves as the cookie's owner and never pays Argon2.
    let (app, pool, basic) = fixture().await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let cookie = auth_test_support::cookie_value(&pool, alice.id).await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;

    let res = app
        .oneshot(get_with_headers(
            "/whoami",
            &[
                (header::COOKIE, cookie),
                (
                    header::AUTHORIZATION,
                    basic_header("basic-reader", PASSWORD),
                ),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "alice");
    assert!(
        basic.verified.lock().await.is_empty(),
        "the Basic credentials must never have been verified"
    );
}

#[tokio::test]
async fn opds_auth_user_re_reads_the_user_row_on_a_cache_hit_so_permission_edits_apply() {
    // Only the Argon2 verify is cached. A `can_download` revoke between two
    // requests must take effect immediately, not after the TTL.
    let (app, pool, basic) = fixture().await;
    let user = auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let request = || {
        get_with_headers(
            "/can-download",
            &[(
                header::AUTHORIZATION,
                basic_header("basic-reader", PASSWORD),
            )],
        )
    };

    let res = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(body_string(res).await, "true");
    assert_eq!(basic.verified.lock().await.len(), 1);

    sqlx::query("UPDATE users SET can_download = 0 WHERE id = ?")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let res = app.oneshot(request()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "false");
}

#[tokio::test]
async fn opds_auth_user_rejects_a_cached_credential_whose_user_row_is_gone() {
    let (app, pool, _basic) = fixture().await;
    let user = auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let request = || {
        get_with_headers(
            "/whoami",
            &[(
                header::AUTHORIZATION,
                basic_header("basic-reader", PASSWORD),
            )],
        )
    };
    let res = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let res = app.oneshot(request()).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge_of(&res).starts_with("Basic realm="));
}

#[tokio::test]
async fn opds_auth_user_returns_500_when_the_pool_closes_after_a_credential_is_cached() {
    // The cache-hit re-read is a DB call of its own; a failure there must
    // surface as a 5xx rather than an anonymous 401 challenge.
    let (app, pool, _basic) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let request = || {
        get_with_headers(
            "/whoami",
            &[(
                header::AUTHORIZATION,
                basic_header("basic-reader", PASSWORD),
            )],
        )
    };
    let res = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    pool.close().await;
    let res = app.oneshot(request()).await.unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn opds_auth_user_returns_500_when_the_basic_auth_state_extension_is_missing() {
    // `opds_router` is the only router that layers it; a route that extracts
    // `OpdsAuthUser` without it is a wiring bug, not an auth failure.
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let app = Router::new()
        .route("/whoami", get(whoami))
        .layer(Extension(pool));
    let res = app
        .oneshot(get_with_headers(
            "/whoami",
            &[(
                header::AUTHORIZATION,
                basic_header("basic-reader", PASSWORD),
            )],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ------------------------------------------------------------ Projection

#[tokio::test]
async fn basic_auth_user_carries_the_sentinel_session_and_the_basic_kind() {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let user = auth_test_support::create_user_with_password(&pool, "basic-reader", PASSWORD).await;
    let auth_user = basic_auth_user(user);
    assert_eq!(auth_user.username, "basic-reader");
    assert!(auth_user.can_download);
    assert_eq!(auth_user.session_id, 0);
    assert_eq!(auth_user.session_kind, SessionKind::Basic);
}
