//! Tests for the three `/api/metadata/*` handlers.
//!
//! Network-free throughout: each case either fails before a provider is
//! reached, or names only an unconfigured one — which the search reports and
//! whose `by_ref` misses without sending. The provider calls themselves are
//! covered by the wiremock suite in `omnibus_db::metadata_lookup`.

use std::time::Duration;

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
    Json,
};
use omnibus_db::{auth::SessionKind, test_support::EnvVarGuard, ThrottleTracker};
use omnibus_shared::metadata_lookup::{
    EditionHydrateRequest, EditionSearchRequest, MetadataProvider,
};
use tower::ServiceExt;

use super::{get_providers, post_edition_hydrate, post_edition_search};
use crate::auth::test_support as auth_test_support;
use crate::auth::AuthUser;
use crate::backend::test_support::*;

const SEARCH_PATH: &str = "/api/metadata/editions/search";
const HYDRATE_PATH: &str = "/api/metadata/editions/hydrate";

/// A minimal `AuthUser` for driving the handler directly (bypassing the
/// `AuthUser` extractor), so a closed pool exercises the handler's own
/// `Err(...) => internal(...)` branch rather than session extraction.
fn fake_user(id: i64) -> AuthUser {
    AuthUser {
        id,
        username: "reader".to_string(),
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        session_id: 1,
        session_kind: SessionKind::Bearer,
    }
}

/// [`fake_user`] with edit permission, for the handler-driven paths that must
/// get past the `can_edit` gate.
fn fake_editor(id: i64) -> AuthUser {
    AuthUser {
        can_edit: true,
        ..fake_user(id)
    }
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn post(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// A search body naming only Hardcover, which is unconfigured in these tests —
/// so the handler answers without any provider ever being contacted.
fn hardcover_only_search(query: &str) -> serde_json::Value {
    serde_json::json!({ "query": query, "providers": ["hardcover"] })
}

#[tokio::test]
async fn api_get_metadata_providers_returns_the_catalog_for_an_authenticated_user() {
    // Pin both keys off so this test's expectations don't drift with a
    // developer's real `.env`.
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None).also_set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/metadata/providers", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    let entries = body.as_array().expect("catalog should be a JSON array");
    assert_eq!(entries.len(), 3, "catalog should list all three providers");

    let hardcover = entries
        .iter()
        .find(|e| e["id"] == "hardcover")
        .expect("catalog should include hardcover");
    assert_eq!(hardcover["configured"], false);
    assert_eq!(hardcover["requires_key"], true);

    let open_library = entries
        .iter()
        .find(|e| e["id"] == "open_library")
        .expect("catalog should include open_library");
    assert_eq!(open_library["configured"], true);
}

#[tokio::test]
async fn api_get_metadata_providers_never_leaks_key_material() {
    let (app, _state, pool) = fixture().await;
    omnibus_db::set_hardcover_api_key(&pool, Some("hc_super_secret_value"))
        .await
        .unwrap();
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/metadata/providers", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(!body.contains("hc_super_secret_value"));
    assert!(!body.contains("masked"));
    assert!(!body.contains("source"));
}

#[tokio::test]
async fn api_get_metadata_providers_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/metadata/providers"))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_providers_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = get_providers(fake_user(1), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── POST /api/metadata/editions/search ───────────────────────────

#[tokio::test]
async fn api_post_edition_search_returns_a_status_row_for_an_unconfigured_provider() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None).also_set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(SEARCH_PATH, &token, hardcover_only_search("dune")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    assert_eq!(body["editions"].as_array().unwrap().len(), 0);
    let sources = body["sources"].as_array().expect("sources is an array");
    assert_eq!(sources.len(), 1, "only the named provider is reported");
    assert_eq!(sources[0]["provider"], "hardcover");
    assert_eq!(sources[0]["display_name"], "Hardcover");
    assert_eq!(
        sources[0]["status"]["kind"], "not_configured",
        "an unconfigured provider must be distinguishable from a clean miss"
    );
}

#[tokio::test]
async fn api_post_edition_search_rejects_a_request_that_asks_nothing() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(SEARCH_PATH, &token, hardcover_only_search("   ")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(body_string(res)
        .await
        .contains("a title, author, or ISBN is required"));
}

#[tokio::test]
async fn api_post_edition_search_rejects_an_oversized_query() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // The scan cap (200 chars), which `EditionSearchRequest::validate`
    // mirrors — not the crate-root `SEARCH_QUERY_MAX_LEN` (1024), which is the
    // FTS/palette cap and shadows this name at the root.
    let long = "x".repeat(omnibus_shared::scan::SEARCH_QUERY_MAX_LEN + 1);
    let res = app
        .oneshot(post(SEARCH_PATH, &token, hardcover_only_search(&long)))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(body_string(res).await.contains("exceeds"));
}

#[tokio::test]
async fn api_post_edition_search_rejects_an_empty_provider_list() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(
            SEARCH_PATH,
            &token,
            serde_json::json!({ "query": "dune", "providers": [] }),
        ))
        .await
        .expect("request should succeed");
    // Searching nothing would render as an outage rather than as the caller's
    // own filter, so it is a 400 instead of an empty 200.
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(body_string(res).await.contains("at least one provider"));
}

#[tokio::test]
async fn api_post_edition_search_requires_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let reader = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, reader.id).await;

    let res = app
        .oneshot(post(SEARCH_PATH, &token, hardcover_only_search("dune")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_edition_search_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let req = Request::builder()
        .uri(SEARCH_PATH)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(hardcover_only_search("dune").to_string()))
        .unwrap();

    let res = app.oneshot(req).await.expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn search_query_prefers_the_structured_fields_when_the_caller_sent_them() {
    // The whole point of the structured request: Open Library gets a
    // `title=`/`author=` pair rather than one flattened phrase searched
    // inside the title field.
    let req = EditionSearchRequest {
        query: "Dune Frank Herbert".to_string(),
        title: Some("Dune".to_string()),
        author: Some("Frank Herbert".to_string()),
        isbn: Some("9780441013593".to_string()),
        providers: None,
    };
    let query = super::search_query(&req);
    assert_eq!(query.title.as_deref(), Some("Dune"));
    assert_eq!(query.author.as_deref(), Some("Frank Herbert"));
    assert_eq!(query.isbn13.as_deref(), Some("9780441013593"));
}

#[test]
fn search_query_reads_free_text_as_a_title_only() {
    // A reader who retyped the box is asking for something specific, and free
    // text cannot honestly be split back into a title and an author.
    let req = EditionSearchRequest {
        query: "dune messiah".to_string(),
        ..EditionSearchRequest::default()
    };
    let query = super::search_query(&req);
    assert_eq!(query.title.as_deref(), Some("dune messiah"));
    assert_eq!(query.author, None);
    assert_eq!(query.isbn13, None);
}

#[test]
fn search_query_falls_back_to_free_text_when_the_structured_fields_are_blank() {
    // The structured fields are *cleaned*, so branching on presence alone
    // would let a client with an empty title field search for nothing at all.
    let req = EditionSearchRequest {
        query: "Dune".to_string(),
        title: Some("   ".to_string()),
        author: Some(String::new()),
        isbn: Some("not-an-isbn".to_string()),
        providers: None,
    };
    let query = super::search_query(&req);
    assert_eq!(query.title.as_deref(), Some("Dune"));
    assert_eq!(query.isbn13, None, "an unusable ISBN is discarded");
}

#[test]
fn search_query_never_copies_free_text_into_the_title_slot() {
    // An author-only search used to backfill `title` from the composed free
    // text, which asked Open Library for a book whose *title* contains the
    // author's name — the precise bug this path exists to remove.
    let req = EditionSearchRequest {
        query: "Frank Herbert".to_string(),
        author: Some("Frank Herbert".to_string()),
        ..EditionSearchRequest::default()
    };
    let query = super::search_query(&req);
    assert_eq!(query.author.as_deref(), Some("Frank Herbert"));
    assert_eq!(query.title, None, "the author must not become the title");
}

#[test]
fn search_query_keeps_an_isbn_only_request_title_less() {
    // Nothing to rank against is handled downstream by `filter_and_rank`,
    // which returns the providers' own results unscored rather than inventing
    // a title to score them by.
    let req = EditionSearchRequest {
        query: String::new(),
        isbn: Some("9780441013593".to_string()),
        ..EditionSearchRequest::default()
    };
    let query = super::search_query(&req);
    assert_eq!(query.isbn13.as_deref(), Some("9780441013593"));
    assert_eq!(query.title, None);
}

#[tokio::test]
async fn api_post_edition_search_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let req = EditionSearchRequest {
        query: "dune".to_string(),
        ..EditionSearchRequest::default()
    };
    let res = post_edition_search(fake_editor(1), State(state), Json(req)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── POST /api/metadata/editions/hydrate ──────────────────────────

/// A hydrate body naming Hardcover, which is unconfigured in these tests — so
/// its `by_ref` answers a clean miss without a request ever leaving.
fn hardcover_hydrate(provider_ref: &str) -> serde_json::Value {
    serde_json::json!({ "source": "hardcover", "provider_ref": provider_ref })
}

#[tokio::test]
async fn api_post_edition_hydrate_answers_null_when_the_provider_does_not_know_the_candidate() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(HYDRATE_PATH, &token, hardcover_hydrate("12345")))
        .await
        .expect("request should succeed");
    // A miss is 200 `null`, never an error: the caller answers it by keeping
    // the list row it already has, and an error would read as one to discard.
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "null");
}

#[tokio::test]
async fn api_post_edition_hydrate_treats_a_malformed_isbn_hint_as_absent() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(
            HYDRATE_PATH,
            &token,
            serde_json::json!({
                "source": "hardcover",
                "provider_ref": "12345",
                "isbn13": "not-an-isbn",
            }),
        ))
        .await
        .expect("request should succeed");
    // The handle is what identifies a candidate; the ISBN is only a hint, so
    // a bad one falls back to the handle rather than stranding the row.
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "null");
}

#[tokio::test]
async fn api_post_edition_hydrate_rejects_a_blank_provider_ref() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(HYDRATE_PATH, &token, hardcover_hydrate("   ")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(body_string(res).await.contains("provider_ref is required"));
}

#[tokio::test]
async fn api_post_edition_hydrate_rejects_an_oversized_provider_ref() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let long = "x".repeat(EditionHydrateRequest::PROVIDER_REF_MAX_LEN + 1);
    let res = app
        .oneshot(post(HYDRATE_PATH, &token, hardcover_hydrate(&long)))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(body_string(res).await.contains("exceeds"));
}

#[tokio::test]
async fn api_post_edition_hydrate_requires_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let reader = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, reader.id).await;

    let res = app
        .oneshot(post(HYDRATE_PATH, &token, hardcover_hydrate("12345")))
        .await
        .expect("request should succeed");
    // Same gate as the search it follows: this is the second outbound call the
    // picker spends, not a cheaper read.
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_edition_hydrate_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let req = Request::builder()
        .uri(HYDRATE_PATH)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(hardcover_hydrate("12345").to_string()))
        .unwrap();

    let res = app.oneshot(req).await.expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// Puts one provider on a cooldown for the length of a test and takes it off
/// again on drop.
///
/// `MetadataLookupConfig::live` reads `ThrottleTracker::shared()` — the
/// process-wide tracker, deliberately, since a cooldown learned serving one
/// request has to be known to the next. Under nextest each test is its own
/// process, but `cargo test` shares one, and a leaked Hardcover cooldown would
/// turn this module's `not_configured` expectations into `throttled`.
struct CooldownGuard(MetadataProvider);

impl CooldownGuard {
    fn set(provider: MetadataProvider) -> Self {
        ThrottleTracker::shared().record(provider, Some(Duration::from_secs(600)));
        Self(provider)
    }
}

impl Drop for CooldownGuard {
    fn drop(&mut self) {
        ThrottleTracker::shared().clear(self.0);
    }
}

#[tokio::test]
async fn api_post_edition_hydrate_returns_502_when_the_provider_could_not_be_asked() {
    // The one branch that separates a failure from a miss, and the distinction
    // the iOS picker depends on: a cooldown short-circuits `hydrate_edition`
    // before any request is built, so this stays network-free.
    let _cooldown = CooldownGuard::set(MetadataProvider::Hardcover);
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(HYDRATE_PATH, &token, hardcover_hydrate("12345")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    // The caller-facing sentence, never the provider's own wording — and never
    // a URL, which is where a `?key=` would ride.
    let body = body_string(res).await;
    assert!(body.contains("temporarily unavailable"), "got: {body}");
    assert!(
        !body.contains("http"),
        "a provider error must not carry a URL"
    );
}

#[tokio::test]
async fn api_post_edition_hydrate_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let req = EditionHydrateRequest {
        source: MetadataProvider::Hardcover,
        provider_ref: "12345".to_string(),
        isbn13: None,
    };
    let res = post_edition_hydrate(fake_editor(1), State(state), Json(req)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
