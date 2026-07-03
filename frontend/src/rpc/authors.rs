//! Author detail fetch, photo resolution / manual upload-by-URL, the authors
//! index, and admin author deletion.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{AuthorDetail, AuthorPhotoScanResult, AuthorSummary};

#[cfg(feature = "server")]
use omnibus_db as db;

// Only `validate_author_photo_url` (server-gated) and its tests reference this
// cap, so keep the import `server`-gated too — otherwise it's an unused import
// in the `mobile`/`web` client builds.
#[cfg(feature = "server")]
use omnibus_shared::AUTHOR_PHOTO_URL_MAX_LEN;

// Magic-byte image-format sniff lives in `omnibus-shared` (single source of
// truth shared with `server::backend`). Only the server-side bodies call it.
#[cfg(feature = "server")]
use omnibus_shared::detect_image_format;

#[cfg(feature = "server")]
use super::{AdminUser, AuthUser, PoolExt, WorkerExt};

/// Fetch a single author and all their books. POST for the same reason as
/// `rpc_get_ebook` — needs `id` in the body.
///
/// Queues a background `Task::ResolveAuthorPhoto` when the author has no
/// `author_photos` row yet so first-time visits trigger Open Library
/// resolution. A subsequent visit picks up the resolved photo (or the
/// `letter` negative-cache marker), and the worker's per-author resource
/// mutex prevents duplicate queueing while resolution is in flight.
#[post("/api/rpc/author", pool: PoolExt, worker: WorkerExt, _user: AuthUser)]
pub async fn rpc_get_author(id: i64) -> Result<Option<AuthorDetail>> {
    let author = db::get_author(&pool.0, id).await?;
    if let Some(ref a) = author {
        if !a.has_photo && db::author_photo_status(&pool.0, id).await?.is_none() {
            worker
                .0
                .post(db::worker::Task::ResolveAuthorPhoto { author_id: id });
        }
    }
    Ok(author)
}

/// Admin-triggered "Scan for picture" for an author. Clears any sticky
/// `letter` negative-cache marker and runs the Open Library resolver
/// inline, so the admin gets a definitive "found / not found" answer in a
/// single round-trip without polling the worker.
///
/// Manual uploads are treated as overrides: a `source = 'manual'` row is
/// preserved and Scan returns `resolved=true` without touching the row,
/// so admins can't accidentally wipe a manual upload by clicking the
/// button.
#[post("/api/rpc/author/scan-photo", pool: PoolExt, user: AuthUser)]
pub async fn rpc_scan_author_photo(id: i64) -> Result<AuthorPhotoScanResult> {
    if !user.is_admin {
        return Err(ServerFnError::new("forbidden: admin required").into());
    }
    // Manual uploads win — don't delete or overwrite.
    if let Some((db::AuthorPhotoSource::Manual, _)) = db::author_photo_status(&pool.0, id).await? {
        return Ok(AuthorPhotoScanResult { resolved: true });
    }
    db::delete_author_photo(&pool.0, id).await?;
    db::author_photos::resolve(&pool.0, id).await?;
    let resolved = db::get_author_photo(&pool.0, id).await?.is_some();
    Ok(AuthorPhotoScanResult { resolved })
}

/// Admin-only: bulk re-resolve all author photos. Posts a single
/// `Task::RefetchAuthorPhotos` to the background worker and returns
/// immediately. Progress is surfaced via the existing `rpc_worker_status`
/// polling loop.
#[post("/api/rpc/refetch-author-photos", _pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_refetch_author_photos() -> Result<()> {
    worker.0.post(omnibus_db::worker::Task::RefetchAuthorPhotos);
    Ok(())
}

/// Validate and trim an author photo URL: non-empty, within
/// `AUTHOR_PHOTO_URL_MAX_LEN` bytes. Returns the trimmed slice on success.
///
/// Only the server-side body of `rpc_set_author_photo_url` calls this, so it
/// is `server`-gated like the other server-only helpers above — otherwise it
/// is dead code in the `mobile`/`web` client builds (caught by clippy).
#[cfg(feature = "server")]
fn validate_author_photo_url(url: &str) -> Result<&str, ServerFnError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("url is required"));
    }
    if trimmed.len() > AUTHOR_PHOTO_URL_MAX_LEN {
        return Err(ServerFnError::new(format!(
            "url must be {} bytes or fewer",
            AUTHOR_PHOTO_URL_MAX_LEN
        )));
    }
    Ok(trimmed)
}

/// Persist an author photo by URL. Admin-gated server-side (the
/// `user.is_admin` check below mirrors `rpc_scan_author_photo`). The
/// server fetches the URL via `db::author_photos::fetch_remote_image`,
/// validates the bytes with the same magic-byte sniff as the multipart
/// upload path, and stores it as a `manual` row.
#[post("/api/rpc/author/photo-url", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_author_photo_url(id: i64, url: String) -> Result<()> {
    if !user.is_admin {
        return Err(ServerFnError::new("forbidden: admin required").into());
    }
    let trimmed = validate_author_photo_url(&url)?;
    let author_exists: bool =
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&pool.0)
            .await
            .map_err(|e| ServerFnError::new(format!("author exists check: {e}")))?
            != 0;
    if !author_exists {
        return Err(ServerFnError::new("author not found").into());
    }
    let (_mime_hint, bytes) = db::author_photos::fetch_remote_image(trimmed)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mime = detect_image_format(&bytes)
        .ok_or_else(|| ServerFnError::new("file at URL does not appear to be a valid image"))?;
    db::upsert_author_photo(
        &pool.0,
        id,
        db::AuthorPhotoSource::Manual,
        Some(trimmed),
        Some(&mime),
        Some(&bytes),
    )
    .await
    .map_err(|e| ServerFnError::new(format!("upsert_author_photo: {e}")))?;
    Ok(())
}

/// `/authors` index: every author across both ebook and audiobook libraries.
#[get("/api/rpc/authors", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_list_authors() -> Result<Vec<AuthorSummary>> {
    let settings = db::get_settings(&pool.0).await?;
    let paths = db::collect_paths(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    );
    Ok(db::list_authors(&pool.0, &paths).await?)
}

/// Admin "Delete author". Removes the author row, drops every
/// `books_authors_link` for it, and adds the name to `ignored_authors`
/// so the next `indexer::reindex` does not silently resurrect the row.
/// Returns the number of books that were un-linked (used by the
/// confirmation modal).
#[post("/api/rpc/author/delete", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_delete_author(id: i64) -> Result<u64> {
    Ok(db::delete_author(&pool.0, id).await?)
}

// `server`-gated alongside `validate_author_photo_url`: these tests exercise
// that helper, which only exists in the server build. CI runs the frontend
// suite as `cargo test -p omnibus-frontend --features server`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{validate_author_photo_url, AUTHOR_PHOTO_URL_MAX_LEN};

    #[test]
    fn validate_author_photo_url_rejects_url_over_max_len() {
        let long_url = "a".repeat(AUTHOR_PHOTO_URL_MAX_LEN + 1);
        let err = validate_author_photo_url(&long_url).unwrap_err();
        assert!(
            err.to_string()
                .contains(&AUTHOR_PHOTO_URL_MAX_LEN.to_string()),
            "error message should name the cap: {err}"
        );
    }

    #[test]
    fn validate_author_photo_url_accepts_url_at_max_len() {
        let at_limit = "a".repeat(AUTHOR_PHOTO_URL_MAX_LEN);
        assert!(validate_author_photo_url(&at_limit).is_ok());
    }

    #[test]
    fn validate_author_photo_url_rejects_empty_url() {
        let err = validate_author_photo_url("").unwrap_err();
        assert!(
            err.to_string().contains("required"),
            "error message should say required: {err}"
        );
    }
}
