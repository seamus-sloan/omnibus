//! Per-book metadata override save/delete (the "edit this book" surface).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{BulkMetadataEdit, EbookMetadata, MetadataOverrides, OpfExportResult};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Save metadata overrides for a book. Requires `can_edit` or admin.
/// Returns the merged `EbookMetadata` so the client can update its state
/// without a second round-trip.
#[post("/api/rpc/ebook/overrides", pool: PoolExt, user: AuthUser)]
pub async fn rpc_save_overrides(
    uuid: String,
    overrides: MetadataOverrides,
) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    if let Err(msg) = overrides.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    // Route through the db layer's read-merge-write (one BEGIN IMMEDIATE) so
    // concurrent edits to the same book can't interleave and drop each other's
    // changes, and a text-only edit keeps the existing cover flag.
    db::merge_metadata_overrides(&pool.0, &uuid, &overrides, user.id)
        .await
        .map_err(|e| internal_rpc_error("save overrides", e))?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?)
}

/// Apply one bulk edit to many books (the table view's bulk-edit modal).
/// Requires `can_edit` or admin. All-or-nothing: the db layer applies every
/// book in one transaction, so a bad uuid edits nothing. Returns the merged
/// `EbookMetadata` for the edited books so the client can update its table
/// rows without a refetch — a book deleted concurrently between the commit
/// and the re-read is omitted from the result rather than failing the
/// already-committed edit.
#[post("/api/rpc/ebook/overrides/bulk", pool: PoolExt, user: AuthUser)]
pub async fn rpc_bulk_save_overrides(
    uuids: Vec<String>,
    edit: BulkMetadataEdit,
) -> Result<Vec<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    Ok(bulk_save_overrides(&pool.0, &uuids, &edit, user.id).await?)
}

/// Server-side body of [`rpc_bulk_save_overrides`], extracted so the
/// validation branches and the merge-then-reload composition can be
/// unit-tested without the server-fn transport.
#[cfg(feature = "server")]
async fn bulk_save_overrides(
    pool: &sqlx::SqlitePool,
    uuids: &[String],
    edit: &BulkMetadataEdit,
    user_id: i64,
) -> Result<Vec<EbookMetadata>, ServerFnError> {
    if uuids.is_empty() {
        return Err(ServerFnError::new("no books selected"));
    }
    omnibus_shared::validate_book_uuids(uuids).map_err(ServerFnError::new)?;
    if edit.is_empty() {
        return Err(ServerFnError::new("no fields to apply"));
    }
    edit.validate().map_err(ServerFnError::new)?;
    db::bulk_merge_metadata_overrides(pool, uuids, edit, user_id)
        .await
        .map_err(|e| match e {
            // Caller-actionable failures keep their own message; everything
            // else is an internal error that must not leak details.
            db::MetadataOverridesError::BookNotFound(_)
            | db::MetadataOverridesError::TooManyTags { .. } => ServerFnError::new(e.to_string()),
            other => internal_rpc_error("bulk save overrides", other),
        })?;
    let mut updated = Vec::with_capacity(uuids.len());
    for uuid in uuids {
        // A book ghosted between the merge and this read is skipped rather
        // than failing the already-committed edit.
        if let Some(book) = db::get_book_by_uuid(pool, uuid)
            .await
            .map_err(|e| internal_rpc_error("get ebook", e))?
        {
            updated.push(book);
        }
    }
    Ok(updated)
}

/// Export a book's merged metadata to its `metadata.opf` sidecar. Requires
/// `can_edit` or admin. Returns the written path and whether an existing
/// sidecar was backed up.
#[post("/api/rpc/ebook/export-opf", pool: PoolExt, user: AuthUser)]
pub async fn rpc_export_opf(uuid: String) -> Result<OpfExportResult> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    let Some(book_id) = db::resolve_book_id_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("resolve book id", e))?
    else {
        return Err(ServerFnError::new("book not found").into());
    };
    match db::export_opf(&pool.0, book_id).await {
        Ok(export) => Ok(OpfExportResult {
            path: export.path.display().to_string(),
            backed_up: export.backed_up,
        }),
        // Stable, user-renderable messages that don't leak the internal
        // numeric book id (which `OpfExportError`'s Display carries) — mirror
        // the REST handler's strings.
        Err(db::OpfExportError::BookNotFound(_)) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::OpfExportError::NoEpubFile(_)) => {
            Err(ServerFnError::new("book has no EPUB file to export next to").into())
        }
        Err(e) => Err(internal_rpc_error("export opf", e).into()),
    }
}

/// Delete metadata overrides for a book, reverting to scanned values.
#[post("/api/rpc/ebook/overrides/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_overrides(uuid: String) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    // Resolve uuid → id once so the `invalidate_thumbs` call (id-keyed by
    // the thumbnail pipeline's file layout) stays accurate.
    let Some(book_id) = db::resolve_book_id_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("resolve book id", e))?
    else {
        return Ok(None);
    };
    db::delete_metadata_overrides(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("delete overrides", e))?;
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so this server function
    // doesn't pin a tokio worker thread (#106).
    let uuid_for_blocking = uuid.clone();
    tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(book_id);
    })
    .await
    .map_err(|e| internal_rpc_error("spawn_blocking(delete_override_cover)", e))?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?)
}

/// Revert an overridden cover back to the scanned original, preserving any
/// other field overrides. The web-only counterpart to the REST `DELETE
/// /api/ebooks/:uuid/cover` route — cover *upload* can't ride this
/// server-function transport (binary body), but the no-body revert can.
#[post("/api/rpc/ebook/cover/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_ebook_cover(uuid: String) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    let Some(book_id) = db::resolve_book_id_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("resolve book id", e))?
    else {
        return Ok(None);
    };
    db::clear_cover_override(&pool.0, &uuid, user.id)
        .await
        .map_err(|e| internal_rpc_error("clear cover override", e))?;
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so this server function
    // doesn't pin a tokio worker thread (#106).
    let uuid_for_blocking = uuid.clone();
    tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(book_id);
    })
    .await
    .map_err(|e| internal_rpc_error("spawn_blocking(delete_override_cover)", e))?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?)
}

// `server`-gated: exercises the extracted server-side body against an
// in-memory DB. CI runs this via `cargo test -p omnibus-frontend --features
// server`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::bulk_save_overrides;
    use omnibus_db::test_support::seed_synced_ebook;
    use omnibus_shared::{BulkMetadataEdit, MetadataOverrides};

    async fn pool_with_admin() -> (sqlx::SqlitePool, i64) {
        let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
        let user_id = omnibus_db::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        (pool, user_id)
    }

    #[tokio::test]
    async fn bulk_save_overrides_returns_the_updated_metadata_for_each_book() {
        let (pool, user_id) = pool_with_admin().await;
        let uuid_a = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;
        let uuid_b = seed_synced_ebook(&pool, "b.epub", "Beta", "Bob Author").await;

        let edit = BulkMetadataEdit {
            publisher: Some("Bulk House".into()),
            add_tags: vec!["bulktag".into()],
            ..Default::default()
        };
        let updated = bulk_save_overrides(&pool, &[uuid_a, uuid_b], &edit, user_id)
            .await
            .unwrap();

        assert_eq!(updated.len(), 2);
        for book in &updated {
            assert_eq!(book.publisher.as_deref(), Some("Bulk House"));
            assert!(book.subjects.iter().any(|t| t == "bulktag"));
            assert!(book.has_override);
        }
    }

    #[tokio::test]
    async fn bulk_save_overrides_rejects_an_empty_uuid_list() {
        let (pool, user_id) = pool_with_admin().await;
        let edit = BulkMetadataEdit {
            publisher: Some("P".into()),
            ..Default::default()
        };
        let err = bulk_save_overrides(&pool, &[], &edit, user_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no books selected"));
    }

    #[tokio::test]
    async fn bulk_save_overrides_rejects_an_empty_edit() {
        let (pool, user_id) = pool_with_admin().await;
        let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;
        let err = bulk_save_overrides(&pool, &[uuid], &BulkMetadataEdit::default(), user_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no fields to apply"));
    }

    #[tokio::test]
    async fn bulk_save_overrides_rejects_an_invalid_edit() {
        let (pool, user_id) = pool_with_admin().await;
        let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;
        let edit = BulkMetadataEdit {
            add_tags: vec!["x".repeat(MetadataOverrides::TAG_MAX_LEN + 1)],
            ..Default::default()
        };
        let err = bulk_save_overrides(&pool, &[uuid], &edit, user_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("tag exceeds"));
    }

    #[tokio::test]
    async fn bulk_save_overrides_surfaces_an_unknown_uuid_without_editing_anything() {
        let (pool, user_id) = pool_with_admin().await;
        let uuid = seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;
        let edit = BulkMetadataEdit {
            publisher: Some("Bulk House".into()),
            ..Default::default()
        };
        let err = bulk_save_overrides(
            &pool,
            &[uuid.clone(), "no-such-uuid".into()],
            &edit,
            user_id,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("book not found"));

        let book = omnibus_db::get_book_by_uuid(&pool, &uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(book.publisher, None, "the known book must be untouched");
    }
}
