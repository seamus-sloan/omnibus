//! F2.3 audiobook file streaming for the in-browser player.
//!
//! `GET /api/audiobooks/{uuid}/file` resolves the uuid to an on-disk
//! audiobook path, then delegates to `tower-http`'s [`ServeFile`] which
//! handles Range / 206 / `If-None-Match` / `If-Modified-Since` natively.
//! Without Range support the browser's `<audio>` element can't seek into a
//! ~400 MB m4b without re-streaming from byte 0; doing the parsing
//! ourselves invites subtle bugs in conditional-GET handling, so we
//! piggyback on tower-http.
//!
//! The handler prefers `M4B` then `M4A` then `MP3` so a single uuid maps
//! cleanly to one playable file when a book ships with both formats —
//! the Listen CTA is uuid-only, not format-specific.

use axum::{
    body::Body,
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
};
use omnibus_db::{self as db};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{internal, AppState};
use crate::auth::AuthUser;

const AUDIOBOOK_FORMATS: &[&str] = &["M4B", "M4A", "MP3"];

/// Stream the raw audiobook bytes for `uuid` with full Range support.
pub(super) async fn get_audiobook_file(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    req: Request,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    let mut resolved: Option<(std::path::PathBuf, &'static str)> = None;
    for fmt in AUDIOBOOK_FORMATS {
        match db::book_file_path(&state.pool, id, fmt).await {
            Ok(Some(path)) => {
                resolved = Some((path, fmt));
                break;
            }
            Ok(None) => continue,
            Err(e) => return internal("book_file_path", e),
        }
    }
    let Some((path, fmt)) = resolved else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };

    // `ServeFile::new(path)` builds a one-shot service; `oneshot(req)`
    // drives the response (Range/conditional-GET handling included). We
    // hand back the wrapped body unchanged and only overlay our own
    // content-type so the browser treats the bytes as audio regardless
    // of how tower-http sniffed the extension.
    let serve = ServeFile::new(path);
    let res = match serve.oneshot(req).await {
        Ok(res) => res,
        Err(e) => return internal("serve audiobook", e),
    };
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(
        axum::http::header::CONTENT_TYPE,
        content_type_for(fmt).parse().expect("static content-type"),
    );
    parts.headers.insert(
        axum::http::header::CACHE_CONTROL,
        "private, max-age=86400".parse().unwrap(),
    );
    parts
        .headers
        .insert(axum::http::header::VARY, "Cookie".parse().unwrap());
    Response::from_parts(parts, Body::new(body))
}

fn content_type_for(format: &str) -> &'static str {
    match format {
        "MP3" => "audio/mpeg",
        _ => "audio/mp4",
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{header::AUTHORIZATION, Request, StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    fn pid_nanos() -> (u32, u128) {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        (pid, nanos)
    }

    async fn seed_one_audiobook(
        pool: &sqlx::SqlitePool,
        format: &str,
        bytes: &[u8],
    ) -> (String, std::path::PathBuf) {
        let (pid, nanos) = pid_nanos();
        let tmp = std::env::temp_dir().join(format!("omnibus_audiobook_test_{pid}_{nanos}"));
        std::fs::create_dir_all(&tmp).unwrap();
        let stem = "the-princess-knight";
        let ext = format.to_ascii_lowercase();
        let file_path = tmp.join(format!("{stem}.{ext}"));
        std::fs::write(&file_path, bytes).unwrap();

        let lib_id =
            sqlx::query("INSERT INTO libraries (path, display_name) VALUES (?, 'audiobooks')")
                .bind(tmp.to_str().unwrap())
                .execute(pool)
                .await
                .unwrap()
                .last_insert_rowid();
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let book_id =
            sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'PK')")
                .bind(uuid)
                .bind(lib_id)
                .bind(tmp.to_str().unwrap())
                .execute(pool)
                .await
                .unwrap()
                .last_insert_rowid();
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime) \
             VALUES (?, ?, ?, ?, '')",
        )
        .bind(book_id)
        .bind(format)
        .bind(stem)
        .bind(bytes.len() as i64)
        .execute(pool)
        .await
        .unwrap();
        (uuid.into(), tmp)
    }

    #[tokio::test]
    async fn api_get_audiobook_file_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/audiobooks/some-uuid/file"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_audiobook_file_returns_404_for_unknown_uuid() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer(
                "/api/audiobooks/does-not-exist/file",
                &token,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_audiobook_file_returns_200_with_m4b_content_type() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let (uuid, tmp) = seed_one_audiobook(&pool, "M4B", b"\0\0\0\x20ftypm4b ").await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/file"),
                &token,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("audio/mp4"),
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn api_get_audiobook_file_honors_range_header_with_206() {
        // Range support is the whole point of going through ServeFile —
        // an audio element seeking into a long file relies on it. Without
        // a 206 response the browser re-downloads from byte 0 on every
        // scrub, which is the bug we'd otherwise ship.
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = b"abcdefghijklmnopqrstuvwxyz".to_vec();
        let (uuid, tmp) = seed_one_audiobook(&pool, "M4B", &body).await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let req = Request::builder()
            .uri(format!("/api/audiobooks/{uuid}/file"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(axum::http::header::RANGE, "bytes=5-9")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], &body[5..=9]);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
