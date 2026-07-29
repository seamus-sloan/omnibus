//! Loopback media server tests — real HTTP round-trips against a
//! port-0 instance, plus the pure helper functions.

use super::*;

/// Boot the router on a random port with a temp downloads/img dir; returns
/// the base URL, the auth token, and the tempdir guard.
async fn boot(files: &[(&str, &str, &[u8])]) -> (String, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("downloads");
    let img_dir = dir.path().join("imgcache");
    std::fs::create_dir_all(&img_dir).expect("img dir");
    for (uuid, rel, bytes) in files {
        let book_dir = root.join(uuid);
        std::fs::create_dir_all(&book_dir).expect("book dir");
        std::fs::write(book_dir.join(rel), bytes).expect("file");
    }
    std::fs::create_dir_all(&root).expect("root");
    let state = Arc::new(MediaState {
        root,
        img_dir,
        token: "sekrit".to_string(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    (
        format!("http://127.0.0.1:{port}"),
        "sekrit".to_string(),
        dir,
    )
}

#[tokio::test]
async fn dl_serves_full_file_with_cors_and_range_headers() {
    let (base, token, _dir) = boot(&[("u1", "book.epub", b"epub-bytes")]).await;
    let resp = reqwest::get(format!("{base}/dl/u1/book.epub?token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
    assert_eq!(resp.headers().get("accept-ranges").unwrap(), "bytes");
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/epub+zip"
    );
    assert_eq!(resp.bytes().await.expect("body").as_ref(), b"epub-bytes");
}

#[tokio::test]
async fn dl_serves_206_slice_with_content_range() {
    let (base, token, _dir) = boot(&[("u1", "part-0.m4b", b"0123456789")]).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/dl/u1/part-0.m4b?token={token}"))
        .header("Range", "bytes=2-5")
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.headers().get("content-range").unwrap(), "bytes 2-5/10");
    assert_eq!(resp.headers().get("content-type").unwrap(), "audio/mp4");
    assert_eq!(resp.bytes().await.expect("body").as_ref(), b"2345");
}

#[tokio::test]
async fn dl_returns_416_for_unsatisfiable_range() {
    let (base, token, _dir) = boot(&[("u1", "part-0.m4b", b"0123456789")]).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/dl/u1/part-0.m4b?token={token}"))
        .header("Range", "bytes=99-")
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), 416);
    assert_eq!(resp.headers().get("content-range").unwrap(), "bytes */10");
}

#[tokio::test]
async fn dl_rejects_bad_token_with_403_and_cors() {
    let (base, _token, _dir) = boot(&[("u1", "book.epub", b"x")]).await;
    let resp = reqwest::get(format!("{base}/dl/u1/book.epub?token=wrong"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 403);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
}

#[tokio::test]
async fn dl_rejects_traversal_segments() {
    let (base, token, _dir) = boot(&[("u1", "book.epub", b"x")]).await;
    let resp = reqwest::get(format!("{base}/dl/x..x/book.epub?token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn img_serves_cached_bytes_with_recorded_content_type() {
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    let name = cache_file_name("/api/thumbs/u1/md");
    std::fs::write(img_dir.join(&name), b"webp-bytes").expect("img");
    std::fs::write(img_dir.join(format!("{name}.ct")), "image/webp").expect("ct");

    let encoded = urlencode("/api/thumbs/u1/md");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("content-type").unwrap(), "image/webp");
    assert_eq!(resp.bytes().await.expect("body").as_ref(), b"webp-bytes");
}

#[tokio::test]
async fn img_404s_when_uncached_and_offline() {
    let (base, token, _dir) = boot(&[]).await;
    let encoded = urlencode("/api/thumbs/u1/md");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 404);
    assert_eq!(
        resp.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
}

#[tokio::test]
async fn img_rejects_disallowed_paths() {
    let (base, token, _dir) = boot(&[]).await;
    let encoded = urlencode("/api/progress/u1");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn img_falls_back_to_cached_cover_for_ungenerated_thumbs() {
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    let name = cache_file_name("/api/covers/u9");
    std::fs::write(img_dir.join(&name), b"cover-bytes").expect("img");
    std::fs::write(img_dir.join(format!("{name}.ct")), "image/jpeg").expect("ct");

    let encoded = urlencode("/api/thumbs/u9/md");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.expect("body").as_ref(), b"cover-bytes");
}

#[test]
fn thumb_cover_fallback_maps_thumb_paths_only() {
    assert_eq!(
        thumb_cover_fallback("/api/thumbs/u1/lg").as_deref(),
        Some("/api/covers/u1")
    );
    assert_eq!(thumb_cover_fallback("/api/covers/u1"), None);
}

#[test]
fn sanitize_segment_allows_uuids_and_file_names_only() {
    assert!(sanitize_segment("0a1b-2c3d_4e"));
    assert!(sanitize_segment("book.epub"));
    assert!(sanitize_segment("part-12.m4b"));
    assert!(!sanitize_segment(""));
    assert!(!sanitize_segment("a/b"));
    assert!(!sanitize_segment("x..x"));
    assert!(!sanitize_segment("a b"));
}

#[test]
fn proxy_path_allowed_is_an_image_read_allowlist() {
    assert!(proxy_path_allowed("/api/covers/u1"));
    assert!(proxy_path_allowed("/api/thumbs/u1/lg"));
    assert!(proxy_path_allowed("/api/authors/9/photo"));
    assert!(proxy_path_allowed("/api/journals/images/pic.png"));
    assert!(proxy_path_allowed("/api/suggestions/u1/2/cover"));
    assert!(!proxy_path_allowed("/api/progress/u1"));
    assert!(!proxy_path_allowed("/api/covers/../auth/me"));
    assert!(!proxy_path_allowed("/api/covers/u1?x=1"));
    assert!(!proxy_path_allowed("/api/authors/9"));
}

#[test]
fn ext_mime_maps_known_media_extensions() {
    assert_eq!(ext_mime("book.epub"), "application/epub+zip");
    assert_eq!(ext_mime("part-0.m4b"), "audio/mp4");
    assert_eq!(ext_mime("part-1.MP3"), "audio/mpeg");
    assert_eq!(ext_mime("weird.xyz"), "application/octet-stream");
}

#[test]
fn urlencode_escapes_reserved_characters() {
    assert_eq!(urlencode("/api/thumbs/u1/md"), "%2Fapi%2Fthumbs%2Fu1%2Fmd");
    assert_eq!(urlencode("a-b_c.d~e"), "a-b_c.d~e");
}

// ── Cover revalidation (#1231 AC4) ─────────────────────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc as StdArc;

use axum::response::IntoResponse;

/// Seed a cache entry and backdate it past [`IMG_REVALIDATE_AFTER_SECS`], so
/// the proxy treats it as due for a check-back.
fn seed_cache_entry(img_dir: &std::path::Path, path: &str, bytes: &[u8], etag: Option<&str>) {
    let name = cache_file_name(path);
    let file = img_dir.join(&name);
    std::fs::write(&file, bytes).expect("img");
    std::fs::write(img_dir.join(format!("{name}.ct")), "image/jpeg").expect("ct");
    if let Some(etag) = etag {
        std::fs::write(img_dir.join(format!("{name}.etag")), etag).expect("etag");
    }
    let stale = std::time::SystemTime::now()
        - std::time::Duration::from_secs(IMG_REVALIDATE_AFTER_SECS + 60);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .expect("open")
        .set_modified(stale)
        .expect("backdate");
}

fn read_cached(img_dir: &std::path::Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(img_dir.join(cache_file_name(path))).ok()
}

/// Spawn an upstream that answers `/api/covers/u1`, counting requests and
/// recording the `If-None-Match` each one carried.
async fn spawn_upstream(
    status: axum::http::StatusCode,
    body: &'static [u8],
    etag: &'static str,
) -> (
    StdArc<AtomicUsize>,
    StdArc<std::sync::Mutex<Option<String>>>,
) {
    let hits = StdArc::new(AtomicUsize::new(0));
    let seen: StdArc<std::sync::Mutex<Option<String>>> = StdArc::new(std::sync::Mutex::new(None));
    let (h, s) = (hits.clone(), seen.clone());
    let app = axum::Router::new().route(
        "/api/covers/u1",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let (h, s) = (h.clone(), s.clone());
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                *s.lock().expect("lock") = headers
                    .get(axum::http::header::IF_NONE_MATCH)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                if status == axum::http::StatusCode::NOT_MODIFIED {
                    return status.into_response();
                }
                (status, [(axum::http::header::ETAG, etag)], body).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    set_upstream(&format!("http://127.0.0.1:{port}"));
    (hits, seen)
}

/// Poll until `f` holds or the budget runs out — the revalidation is
/// detached, so there is nothing to await directly.
async fn eventually(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if f() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn img_serves_the_cached_copy_then_replaces_it_when_the_cover_changed() {
    // AC4: a cover edited on another device reaches this one without the
    // reader doing anything — and without the request that noticed paying
    // for it.
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    seed_cache_entry(&img_dir, "/api/covers/u1", b"old-cover", Some("\"v1\""));
    let (hits, seen) =
        spawn_upstream(axum::http::StatusCode::OK, b"new-cover-bytes", "\"v2\"").await;

    let encoded = urlencode("/api/covers/u1");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.bytes().await.expect("body").as_ref(),
        b"old-cover",
        "the request that triggers revalidation still serves instantly"
    );

    assert!(
        eventually(
            || read_cached(&img_dir, "/api/covers/u1").as_deref() == Some(&b"new-cover-bytes"[..])
        )
        .await,
        "the replaced cover must land in the cache for the next render"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen.lock().expect("lock").as_deref(),
        Some("\"v1\""),
        "revalidation must offer the stored validator"
    );
    // The new validator replaces the old one, so the next check-back asks
    // about the bytes actually held.
    let name = cache_file_name("/api/covers/u1");
    assert!(
        eventually(|| {
            std::fs::read_to_string(img_dir.join(format!("{name}.etag")))
                .is_ok_and(|s| s == "\"v2\"")
        })
        .await
    );
}

#[tokio::test]
async fn img_keeps_the_cached_copy_when_the_server_answers_not_modified() {
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    seed_cache_entry(&img_dir, "/api/covers/u1", b"same-cover", Some("\"v1\""));
    let (hits, _seen) = spawn_upstream(axum::http::StatusCode::NOT_MODIFIED, b"", "\"v1\"").await;

    let encoded = urlencode("/api/covers/u1");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    assert_eq!(resp.bytes().await.expect("body").as_ref(), b"same-cover");

    assert!(eventually(|| hits.load(Ordering::SeqCst) == 1).await);
    // Give the (no-op) revalidation time to have done damage if it were
    // going to, then confirm it didn't.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        read_cached(&img_dir, "/api/covers/u1").as_deref(),
        Some(&b"same-cover"[..]),
        "a 304 must leave the cached bytes exactly as they were"
    );
}

#[tokio::test]
async fn img_does_not_revalidate_a_freshly_cached_image() {
    // Without a fresh window, scrolling a grid would fire one conditional
    // request per visible cover every time it came into view.
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    let name = cache_file_name("/api/covers/u1");
    std::fs::write(img_dir.join(&name), b"fresh-cover").expect("img");
    std::fs::write(img_dir.join(format!("{name}.ct")), "image/jpeg").expect("ct");
    std::fs::write(img_dir.join(format!("{name}.etag")), "\"v1\"").expect("etag");
    let (hits, _seen) = spawn_upstream(axum::http::StatusCode::OK, b"new", "\"v2\"").await;

    let encoded = urlencode("/api/covers/u1");
    let _ = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn img_does_not_revalidate_an_entry_with_no_stored_validator() {
    // Nothing to offer as `If-None-Match`, so a "revalidation" would be a
    // full refetch — as expensive as not caching at all.
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    seed_cache_entry(&img_dir, "/api/covers/u1", b"legacy-cover", None);
    let (hits, _seen) = spawn_upstream(axum::http::StatusCode::OK, b"new", "\"v2\"").await;

    let encoded = urlencode("/api/covers/u1");
    let _ = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fetch_and_cache_records_the_validator_for_the_next_revalidation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let img_dir = dir.path().to_path_buf();
    let (_hits, _seen) = spawn_upstream(axum::http::StatusCode::OK, b"cover-bytes", "\"v7\"").await;

    match fetch_and_cache(&img_dir, "/api/covers/u1", None).await {
        Fetched::Replaced { bytes, .. } => assert_eq!(bytes, b"cover-bytes"),
        _ => panic!("expected new bytes"),
    }

    let cached = cached_image(&img_dir, "/api/covers/u1")
        .await
        .expect("cached");
    assert_eq!(cached.bytes, b"cover-bytes");
    assert_eq!(cached.etag.as_deref(), Some("\"v7\""));
}

#[tokio::test]
async fn fetch_and_cache_drops_a_stale_validator_when_the_server_stops_sending_one() {
    // Replaying a validator the server no longer publishes would let a
    // later 304 vouch for bytes it never saw.
    let dir = tempfile::tempdir().expect("tempdir");
    let img_dir = dir.path().to_path_buf();
    let name = cache_file_name("/api/covers/u1");
    std::fs::write(img_dir.join(format!("{name}.etag")), "\"stale\"").expect("etag");

    let app = axum::Router::new().route(
        "/api/covers/u1",
        axum::routing::get(|| async { b"no-validator-here".to_vec() }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    set_upstream(&format!("http://127.0.0.1:{port}"));

    let _ = fetch_and_cache(&img_dir, "/api/covers/u1", None).await;
    let cached = cached_image(&img_dir, "/api/covers/u1")
        .await
        .expect("cached");
    assert_eq!(cached.etag, None);
}

#[tokio::test]
async fn img_never_lets_the_webview_cache_past_the_revalidation_window() {
    // A `max-age` here is what made the five-minute window unreachable: the
    // WebView honoured it, so re-rendering a cover never reached this
    // handler, revalidation never ran, and the one render that did reach it
    // handed back the *old* bytes with another day of freshness. A cover
    // replaced elsewhere could take two days or a restart to appear.
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    seed_cache_entry(&img_dir, "/api/covers/u1", b"cover", Some("\"v1\""));

    let encoded = urlencode("/api/covers/u1");
    let resp = reqwest::get(format!("{base}/img?path={encoded}&token={token}"))
        .await
        .expect("get");
    let cache_control = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        !cache_control.contains("max-age"),
        "a max-age would put the WebView's copy beyond reach; got {cache_control:?}"
    );
    assert!(
        resp.headers().get("etag").is_some(),
        "asking every render is only cheap because an unchanged image answers 304"
    );
}

#[tokio::test]
async fn img_answers_a_webview_re_render_with_304() {
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    seed_cache_entry(&img_dir, "/api/covers/u1", b"cover", Some("\"v1\""));
    let (_hits, _seen) = spawn_upstream(axum::http::StatusCode::NOT_MODIFIED, b"", "\"v1\"").await;

    let encoded = urlencode("/api/covers/u1");
    let url = format!("{base}/img?path={encoded}&token={token}");
    let first = reqwest::get(&url).await.expect("get");
    let etag = first
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("etag")
        .to_string();

    let second = reqwest::Client::new()
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .expect("get");
    assert_eq!(second.status(), 304);
    assert!(
        second.bytes().await.expect("body").is_empty(),
        "the WebView keeps its decoded image; the round-trip carries no body"
    );
}

#[tokio::test]
async fn a_304_from_upstream_restarts_the_freshness_window() {
    // Without this, an entry the server confirmed was current stayed
    // "overdue" forever, so every later render fired another conditional
    // request — the in-flight guard only collapses overlapping checks, not
    // sequential ones.
    let (base, token, dir) = boot(&[]).await;
    let img_dir = dir.path().join("imgcache");
    seed_cache_entry(&img_dir, "/api/covers/u1", b"cover", Some("\"v1\""));
    let (hits, _seen) = spawn_upstream(axum::http::StatusCode::NOT_MODIFIED, b"", "\"v1\"").await;

    let encoded = urlencode("/api/covers/u1");
    let url = format!("{base}/img?path={encoded}&token={token}");

    let _ = reqwest::get(&url).await.expect("get");
    assert!(eventually(|| hits.load(Ordering::SeqCst) == 1).await);

    // The window has been restarted, so further renders ask nothing.
    for _ in 0..3 {
        let _ = reqwest::get(&url).await.expect("get");
    }
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a confirmed-current entry must not be re-asked on every render"
    );
}

#[tokio::test]
async fn prune_removes_a_cache_entry_whole_or_not_at_all() {
    // The bytes, the `.ct` and the `.etag` are one logical entry, and their
    // mtimes drift apart because a 304 touches only the bytes. Pruning them
    // as independent files deletes an entry's validator while keeping its
    // image — and an image with no validator is skipped by revalidation
    // forever, which is the stale cover this cache exists to prevent.
    let dir = tempfile::tempdir().expect("tempdir");
    let img_dir = dir.path();

    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
    for (path, bytes) in [("/api/covers/keep", 400_usize), ("/api/covers/drop", 400)] {
        let name = cache_file_name(path);
        std::fs::write(img_dir.join(&name), vec![0u8; bytes]).expect("img");
        std::fs::write(img_dir.join(format!("{name}.ct")), "image/jpeg").expect("ct");
        std::fs::write(img_dir.join(format!("{name}.etag")), "\"v1\"").expect("etag");
        // Backdate every sidecar, exactly as a 304-touched entry looks.
        for sidecar in [format!("{name}.ct"), format!("{name}.etag")] {
            std::fs::File::options()
                .write(true)
                .open(img_dir.join(sidecar))
                .expect("open")
                .set_modified(old)
                .expect("backdate");
        }
    }
    // Make one entry the older of the two, so it is the one evicted.
    let dropped = cache_file_name("/api/covers/drop");
    std::fs::File::options()
        .write(true)
        .open(img_dir.join(&dropped))
        .expect("open")
        .set_modified(old)
        .expect("backdate");

    prune_img_cache(img_dir, 500);

    let kept = cache_file_name("/api/covers/keep");
    assert!(img_dir.join(&kept).exists(), "the newer entry survives");
    assert!(
        img_dir.join(format!("{kept}.etag")).exists(),
        "a survivor must keep the validator it needs to revalidate with"
    );
    assert!(img_dir.join(format!("{kept}.ct")).exists());
    // And the evicted one leaves nothing behind.
    assert!(!img_dir.join(&dropped).exists());
    assert!(!img_dir.join(format!("{dropped}.etag")).exists());
    assert!(!img_dir.join(format!("{dropped}.ct")).exists());
}

#[tokio::test]
async fn writes_to_one_cache_key_are_serialized() {
    // The foreground `?v=` bust and a background revalidation used to write
    // the same entry — and, before this, the same temp file — concurrently.
    let held = lock_cache_key("/api/covers/u1").await;

    let entered = StdArc::new(AtomicUsize::new(0));
    let probe = entered.clone();
    let waiter = tokio::spawn(async move {
        let _guard = lock_cache_key("/api/covers/u1").await;
        probe.fetch_add(1, Ordering::SeqCst);
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        entered.load(Ordering::SeqCst),
        0,
        "a second writer for the same key must wait"
    );

    drop(held);
    waiter.await.expect("join");
    assert_eq!(entered.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_different_cache_key_is_not_blocked() {
    let _held = lock_cache_key("/api/covers/u1").await;
    // Would hang if the lock were global rather than per key.
    let _other = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        lock_cache_key("/api/covers/u2"),
    )
    .await
    .expect("a different image must not queue behind this one");
}
