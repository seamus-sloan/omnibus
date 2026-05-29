// ---------------------------------------------------------------------------
// F5.1 Metadata overrides (REST — mobile client).
// ---------------------------------------------------------------------------

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::{detect_image_format, MetadataOverrides};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Save metadata overrides for a book. Requires `can_edit` or admin.
pub(super) async fn post_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(overrides): Json<MetadataOverrides>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    if let Err(msg) = overrides.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    // Resolve uuid → id so the thumbnail/cover invalidate calls stay
    // id-keyed. Returns 404 for unknown uuids — same behavior the old
    // `get_book_uuid(id)` -> 404 had for unknown ids.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    // Merge incoming overrides with any existing ones so a second edit that
    // only touches field B doesn't wipe a prior override on field A. The
    // read-merge-write is serialized inside a single BEGIN IMMEDIATE
    // transaction in the db layer so two concurrent edits to the same book
    // can't interleave and drop each other's changes (#166).
    if let Err(e) = db::merge_metadata_overrides(&state.pool, &uuid, &overrides, user.id).await {
        return internal("merge_metadata_overrides", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Delete metadata overrides for a book, reverting to scanned values.
pub(super) async fn delete_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    if let Err(e) = db::delete_metadata_overrides(&state.pool, &uuid).await {
        return internal("delete_metadata_overrides", e);
    }
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so the axum runtime stays
    // responsive under load (#106).
    let uuid_for_blocking = uuid.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(id);
    })
    .await
    {
        return internal("spawn_blocking(delete_override_cover)", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Upload a replacement cover image for a book. Multipart form with a single
/// `cover` field containing the image bytes.
pub(super) async fn post_ebook_cover(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }

    // uuid → id for the thumb-invalidate + `get_book` calls that still
    // use the internal autoincrement key. Returns 404 for unknown uuids,
    // matching the prior `get_book_uuid(id)` → 404 behavior.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    // Extract the cover field from the multipart body.
    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "cover" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "cover must be an image",
                    )
                        .into_response();
                }
                // Reject SVG — contains executable content and can XSS when
                // opened directly in a browser tab.
                if content_type.contains("svg") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "SVG covers are not accepted",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "cover must be under 10 MB",
                            )
                                .into_response();
                        }
                        // Validate magic bytes — don't trust Content-Type alone.
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            // Use the detected MIME so the stored extension matches
                            // actual content, not the (untrusted) client header.
                            Some(mime) => break (mime, b),
                            None => {
                                return (
                                    axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                    "Could not detect image format",
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => return internal("read cover field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'cover' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    // Write the override cover to disk. `write_override_cover` is a sync
    // `std::fs` call — run it on the blocking pool so the axum runtime stays
    // responsive while we hit disk (#106). `uuid` is needed again below for
    // the overrides table update, so it's the only value we clone.
    let uuid_for_write = uuid.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        db::write_override_cover(&uuid_for_write, &mime, &bytes)
    })
    .await;
    match write_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return internal("write_override_cover", e),
        Err(e) => return internal("spawn_blocking(write_override_cover)", e),
    }

    // Mark the overrides table with has_cover_override = 1. Preserve existing
    // field overrides if any.
    let existing_overrides = match db::get_metadata_overrides(&state.pool, &uuid).await {
        Ok(Some((ov, _))) => ov,
        Ok(None) => MetadataOverrides::default(),
        Err(e) => return internal("get_metadata_overrides", e),
    };
    if let Err(e) =
        db::upsert_metadata_overrides(&state.pool, &uuid, &existing_overrides, true, user.id).await
    {
        return internal("upsert_metadata_overrides", e);
    }

    // Invalidate thumb cache so next request regenerates from new cover.
    // Also sync `std::fs` — run on the blocking pool (#106).
    if let Err(e) = tokio::task::spawn_blocking(move || db::thumbs::invalidate_thumbs(id)).await {
        return internal("spawn_blocking(invalidate_thumbs)", e);
    }

    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

#[cfg(test)]
mod tests {
    // -------------------------------------------------------------------
    // F5.1 metadata-override REST endpoints (issue #105).
    //
    // The RPC variants (`rpc_save_overrides`, `rpc_delete_overrides`) are
    // covered by DB-level unit tests in `db::queries`; these integration
    // tests cover the REST entry points the mobile client uses.
    // -------------------------------------------------------------------

    use axum::{
        body::{to_bytes, Body},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    #[tokio::test]
    async fn api_post_overrides_requires_auth() {
        let (app, _state, _pool) = fixture().await;
        let body = serde_json::json!({ "title": "Edited" });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/ebooks/1/overrides")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_post_overrides_requires_edit_permission() {
        // A plain user from `create_user` has `can_edit = false`, so the
        // handler's per-route check must reject them with 403 before the
        // override row is touched.
        let (app, _state, pool) = fixture().await;
        let id = seed_book(&pool, "/lib", "Original").await;
        let user = auth_test_support::create_user(&pool, "reader").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let body = serde_json::json!({ "title": "Edited" });
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{id}/overrides"))
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // No override row should have been written.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "403 path must not persist any override"
        );
    }

    #[tokio::test]
    async fn api_post_overrides_saves_and_returns_merged_book() {
        // Admin (which carries `can_edit = true` via test_support::create_admin)
        // POSTs an override. The handler must persist it, return the merged
        // book, and flip `has_override` on the response.
        let (app, _state, pool) = fixture().await;
        let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        let body = serde_json::json!({
            "title": "Edited Title",
            "publisher": "Edited Publisher",
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/overrides"))
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Edited Title"));
        assert_eq!(book.publisher.as_deref(), Some("Edited Publisher"));
        assert!(
            book.has_override,
            "merged book should advertise has_override = true"
        );

        // The override row must reflect the saved fields.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let (saved, has_cover) = db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .expect("override row should exist after POST");
        assert_eq!(saved.title.as_deref(), Some("Edited Title"));
        assert_eq!(saved.publisher.as_deref(), Some("Edited Publisher"));
        assert!(!has_cover, "text-only edit must not set has_cover_override");
    }

    #[tokio::test]
    async fn api_delete_overrides_reverts() {
        // Persist an override via the same REST path the client uses, then
        // delete it and assert the response reflects the canonical scanned
        // values (no `Option` overrides applied).
        let (app, _state, pool) = fixture().await;
        let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        let post_body = serde_json::json!({ "title": "Edited" });
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/overrides"))
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(post_body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("POST should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/overrides"))
                    .method("DELETE")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("DELETE should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.id, id);
        assert_eq!(
            book.title.as_deref(),
            Some("Original"),
            "delete must revert to the scanned title"
        );
        assert!(
            !book.has_override,
            "delete must clear the has_override flag on the merged book"
        );

        // And the override row must be gone from the DB.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "delete must drop the metadata_overrides row"
        );
    }

    #[tokio::test]
    async fn api_post_cover_upload_replaces_cover() {
        // End-to-end happy path: admin POSTs a valid PNG as multipart and
        // the handler writes the override cover file, flips
        // `has_cover_override` on the row, and returns the merged book with
        // `has_override = true`.
        let _covers = CoversDirGuard::new("upload_replaces");
        let (app, _state, pool) = fixture().await;
        let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "CoverBook").await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_cover_multipart("image/png", TINY_PNG);
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.id, id);
        assert!(
            book.has_override,
            "uploading a cover must mark the merged book as overridden"
        );

        // The override row must record `has_cover_override = 1` and the
        // PNG bytes must be on disk under the scratch covers dir.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let (_, has_cover_override) = db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .expect("override row should exist after cover upload");
        assert!(
            has_cover_override,
            "cover upload must set has_cover_override = 1"
        );
        let override_path = db::covers_dir().join(format!("override-{uuid}.png"));
        let on_disk = std::fs::read(&override_path).expect("override cover file should be on disk");
        assert_eq!(on_disk, TINY_PNG);
    }

    #[tokio::test]
    async fn api_post_cover_rejects_non_image() {
        // A multipart `cover` field whose Content-Type is `text/plain` must
        // be rejected at the `starts_with("image/")` guard with 400.
        let _covers = CoversDirGuard::new("non_image");
        let (app, _state, pool) = fixture().await;
        let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "NonImageBook").await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_cover_multipart("text/plain", b"not an image");
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // No override row should have been written for the rejected upload.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "rejected non-image upload must not create an override row"
        );
    }

    #[tokio::test]
    async fn api_post_cover_rejects_oversized() {
        // The per-route layer raises the body cap to 11 MiB so the handler
        // can enforce its own 10 MB content cap with a clean 400 instead of
        // the framework's 413. Build a payload just over 10 MB to trip that
        // handler-level check.
        let _covers = CoversDirGuard::new("oversized");
        let (app, _state, pool) = fixture().await;
        let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "OversizedBook").await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        // 10 MiB + 1 KiB of PNG-prefixed bytes — passes magic-byte
        // detection so we reach the size check, not the format check.
        let mut payload = TINY_PNG.to_vec();
        payload.resize(10 * 1024 * 1024 + 1024, 0);
        let (content_type, body) = build_cover_multipart("image/png", &payload);
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap_or("");
        assert!(
            body.contains("10 MB"),
            "400 body should explain the size cap, got {body:?}"
        );
    }

    #[tokio::test]
    async fn api_post_cover_rejects_undetectable_format() {
        // Content-Type passes the `image/` prefix gate but the bytes carry no
        // recognisable image magic header, so `detect_image_format` returns
        // `None`. The handler must surface a 415 (#210) instead of
        // `.unwrap()`-panicking the task into a bare 500.
        let _covers = CoversDirGuard::new("undetectable_format");
        let (app, _state, pool) = fixture().await;
        let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "UndetectableBook").await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        // image/png header, but the body is not a real image.
        let (content_type, body) =
            build_cover_multipart("image/png", b"definitely not image bytes");
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap_or("");
        assert!(
            body.contains("Could not detect image format"),
            "415 body should explain the format detection failure, got {body:?}"
        );

        // No override row should have been written for the rejected upload.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "rejected undetectable-format upload must not create an override row"
        );
    }
}
