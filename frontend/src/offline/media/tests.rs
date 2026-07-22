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
