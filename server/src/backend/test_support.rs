//! Shared test fixtures and helpers for `/api/*` REST integration tests.
//!
//! The `*DirGuard` types below hold a `std::sync::MutexGuard` across
//! `.await` points in their callers (#1169) — sound only because
//! `#[tokio::test]` defaults to the current-thread runtime, so this OS
//! thread never contends with another task for the same lock; a
//! multi-threaded runtime blocking a worker on a std `Mutex` mid-`.await`
//! could stall or deadlock unrelated tasks. `auth::boot::tests::EnvGuard`
//! relies on the same invariant.
use axum::{
    body::Body,
    http::{header::AUTHORIZATION, Request},
};

use super::*;

/// Asserts the calling test runs on tokio's current-thread flavor — the
/// invariant every `*DirGuard` in this module (and `EnvGuard` in
/// `auth::boot::tests`) relies on to hold its lock across `.await` (#1169).
/// Call this as the first line of a guard's constructor rather than only
/// documenting the assumption, so a future switch to a multi-threaded test
/// runtime fails loudly instead of silently reintroducing a deadlock risk.
pub(crate) fn assert_current_thread_test_runtime() {
    debug_assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread,
        "holding a std::sync::MutexGuard across .await is only sound on tokio's \
         current-thread runtime — see backend::test_support module doc (#1169)"
    );
}

/// Build a router + AppState wired against a fresh in-memory DB.
pub(crate) async fn fixture() -> (Router, AppState, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:")
        .await
        .expect("db should initialize");
    let state = AppState::new(pool.clone());
    let app = rest_router(state.clone());
    (app, state, pool)
}

/// Variant of [`fixture`] that flips the SSRF guard off so the
/// wiremock-backed `put_author_photo_url` tests can drive the handler
/// against a server bound to `127.0.0.1`. Production paths
/// always construct `AppState::new` and therefore always block private
/// IPs.
pub(crate) async fn fixture_loopback_remote_image() -> (Router, AppState, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:")
        .await
        .expect("db should initialize");
    let state = AppState::new_with_remote_image_config(
        pool.clone(),
        db::author_photos::RemoteImageConfig {
            allow_private_addresses: true,
        },
    );
    let app = rest_router(state.clone());
    (app, state, pool)
}

/// Convenience: GET request with a bearer auth header.
pub(crate) fn get_with_bearer(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// Convenience: anonymous GET (no auth header).
pub(crate) fn get_anon(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

/// Seed a book row with `has_cover = 0`. Returns the inserted book id.
pub(crate) async fn seed_book_no_cover(pool: &sqlx::SqlitePool) -> i64 {
    // Insert a minimal scan_roots row first (FK requirement).
    sqlx::query(
        "INSERT OR IGNORE INTO scan_roots(path, display_name) VALUES ('/test/library', 'Test')",
    )
    .execute(pool)
    .await
    .expect("insert library");
    let library_id: i64 =
        sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/test/library'")
            .fetch_one(pool)
            .await
            .expect("library id");
    // Use a fixed UUID; each test gets its own in-memory pool so there is
    // no collision risk.
    sqlx::query(
        "INSERT INTO books(uuid, library_id, path, title, has_cover) VALUES (?, ?, ?, ?, 0)",
    )
    .bind("00000000-0000-0000-0000-000000000001")
    .bind(library_id)
    .bind("/test/library/no-cover.epub")
    .bind("No Cover Book")
    .execute(pool)
    .await
    .expect("insert book")
    .last_insert_rowid()
}

/// Seed a single book with a known title via `replace_books` and return
/// its `id`. The book's stable UUID can be looked up afterwards via
/// `list_books` if a test needs to assert against the overrides row
/// directly.
pub(crate) async fn seed_book(pool: &sqlx::SqlitePool, library: &str, title: &str) -> i64 {
    seed_book_with_uuid(pool, library, title).await.0
}

/// Same as `seed_book` but returns `(id, uuid)`. New tests that build
/// uuid-keyed URLs (covers, thumbs, ebooks, overrides) use this.
pub(crate) async fn seed_book_with_uuid(
    pool: &sqlx::SqlitePool,
    library: &str,
    title: &str,
) -> (i64, String) {
    db::replace_books(
        pool,
        library,
        vec![db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: format!("{title}.epub").to_lowercase(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        }],
    )
    .await
    .expect("seed_book should succeed");
    let books = db::list_books(pool, library).await.expect("list_books");
    let book = books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .expect("seeded book should be present");
    (book.id, book.unique_identifier.clone().unwrap())
}

/// Build a `multipart/form-data` body with a single `cover` field
/// carrying `bytes` under the supplied content type. Returns both the
/// `Content-Type` header value (with the boundary parameter) and the
/// body bytes so the caller can attach them to a `Request::builder()`.
pub(crate) fn build_cover_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----omnibus-test-boundary-XYZ123";
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"cover\"; filename=\"cover.png\"\r\n",
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Minimal 1x1 transparent PNG used as a stand-in real image payload —
/// `detect_image_format` only inspects the leading magic bytes, so this
/// is sufficient to flow through the upload path without bundling a
/// fixture file.
pub(crate) const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// Process-global `OMNIBUS_COVERS_DIR` lock — the cover-upload tests
/// each install their own scratch dir via `set_var`, so they must
/// serialize with each other (and with anything else in this crate
/// that swaps the same env var). Mirrors the `COVERS_ENV_LOCK` in
/// `db::queries::tests`.
pub(crate) static COVER_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that points `OMNIBUS_COVERS_DIR` at a fresh scratch dir
/// for the duration of a single test and restores the previous value
/// (or removes the var) on drop. Holds the `COVER_DIR_ENV_LOCK` so
/// parallel cover tests serialize their env-var writes.
pub(crate) struct CoversDirGuard {
    path: std::path::PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl CoversDirGuard {
    pub(crate) fn new(tag: &str) -> Self {
        assert_current_thread_test_runtime();
        let guard = COVER_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("omnibus_rest_covers_{tag}_{pid}_{nanos}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create covers scratch dir");
        let prev = std::env::var("OMNIBUS_COVERS_DIR").ok();
        std::env::set_var("OMNIBUS_COVERS_DIR", &path);
        Self {
            path,
            prev,
            _guard: guard,
        }
    }
}

impl Drop for CoversDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        match self.prev.take() {
            Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
            None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
        }
    }
}

/// Process-global `OMNIBUS_THUMBS_DIR` lock — the thumb-serving tests each
/// install their own scratch dir via `set_var`, so they must serialize with
/// each other. Mirrors [`COVER_DIR_ENV_LOCK`].
pub(crate) static THUMBS_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that points `OMNIBUS_THUMBS_DIR` at a fresh scratch dir for
/// the duration of a single test and restores the previous value (or
/// removes the var) on drop. Mirrors [`CoversDirGuard`].
pub(crate) struct ThumbsDirGuard {
    path: std::path::PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl ThumbsDirGuard {
    pub(crate) fn new(tag: &str) -> Self {
        assert_current_thread_test_runtime();
        let guard = THUMBS_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("omnibus_rest_thumbs_{tag}_{pid}_{nanos}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create thumbs scratch dir");
        let prev = std::env::var("OMNIBUS_THUMBS_DIR").ok();
        std::env::set_var("OMNIBUS_THUMBS_DIR", &path);
        Self {
            path,
            prev,
            _guard: guard,
        }
    }
}

impl Drop for ThumbsDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        match self.prev.take() {
            Some(v) => std::env::set_var("OMNIBUS_THUMBS_DIR", v),
            None => std::env::remove_var("OMNIBUS_THUMBS_DIR"),
        }
    }
}

/// Process-global `OMNIBUS_DATA_DIR` lock — the KEPUB-download tests point
/// the cache root at a scratch dir via `set_var`, so they must serialize with
/// each other. Mirrors [`COVER_DIR_ENV_LOCK`].
pub(crate) static DATA_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that points `OMNIBUS_DATA_DIR` (the KEPUB cache root lives at
/// `<data_dir>/kepub/`) at a fresh scratch dir for one test and restores the
/// previous value on drop. `path` is public so a test can pre-populate the
/// cache. Holds [`DATA_DIR_ENV_LOCK`] so parallel tests serialize their
/// writes (module doc explains why holding it across `.await` is safe).
pub(crate) struct DataDirGuard {
    pub(crate) path: std::path::PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl DataDirGuard {
    pub(crate) fn new(tag: &str) -> Self {
        assert_current_thread_test_runtime();
        let guard = DATA_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("omnibus_rest_data_{tag}_{pid}_{nanos}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create data scratch dir");
        let prev = std::env::var("OMNIBUS_DATA_DIR").ok();
        std::env::set_var("OMNIBUS_DATA_DIR", &path);
        Self {
            path,
            prev,
            _guard: guard,
        }
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        match self.prev.take() {
            Some(v) => std::env::set_var("OMNIBUS_DATA_DIR", v),
            None => std::env::remove_var("OMNIBUS_DATA_DIR"),
        }
    }
}

pub(crate) async fn seed_author(pool: &sqlx::SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed author")
}

/// Multipart body with one `photo` field, matching `build_cover_multipart`
/// but using the field name the author-photo handler expects.
pub(crate) fn build_photo_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----omnibus-test-photo-boundary";
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"photo\"; filename=\"photo.png\"\r\n",
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// JSON PUT helper for the `/api/authors/:id/photo/url` route.
pub(crate) fn put_photo_url(uri: &str, token: &str, url: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("PUT")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(serde_json::json!({ "url": url }).to_string()))
        .unwrap()
}
