//! Cover-override file I/O (write/clear/delete the on-disk override cover)
//! plus the overrides→`EbookMetadata` read merge — colocated because
//! [`apply_overrides`] is what decides `has_cover_override`/`cover_url` for
//! every read path, mirroring the write side's cover handling here.

use omnibus_shared::{EbookMetadata, MetadataOverrides, MetadataSource};
use sqlx::SqlitePool;

use crate::books::resolve_book_id_by_uuid_exec;
use crate::sync::upsert_fts;

use super::{
    invalidate_export_epub_cache_for, touch_book_last_modified, upsert_overrides_row,
    MetadataOverridesError,
};

/// Clear a book's cover override, reverting `cover_url` to the scanned
/// original while preserving any text-field overrides that remain. A no-op
/// if the book has no cover override active.
///
/// When no text overrides remain either, the whole `metadata_overrides` row
/// is deleted (mirrors [`super::delete_metadata_overrides`]'s FTS-restore
/// step) so `has_override` reads false again — otherwise an empty-but-present
/// row would leave the "Override active" indicator stuck on.
pub async fn clear_cover_override(
    pool: &SqlitePool,
    book_uuid: &str,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let existing: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((json, has_cover)) = existing else {
        tx.commit().await?;
        return Ok(());
    };
    if has_cover == 0 {
        tx.commit().await?;
        return Ok(());
    }

    let overrides: MetadataOverrides = serde_json::from_str(&json)?;
    let mut cleared_book_id = None;
    if overrides == MetadataOverrides::default() {
        sqlx::query("DELETE FROM metadata_overrides WHERE book_uuid = ?")
            .bind(book_uuid)
            .execute(&mut *tx)
            .await?;
        // Resolve inside the transaction so the id read agrees with the DELETE.
        cleared_book_id = resolve_book_id_by_uuid_exec(&mut *tx, book_uuid).await?;
        if let Some(book_id) = cleared_book_id {
            upsert_fts(&mut tx, book_id).await?;
        }
    } else {
        upsert_overrides_row(&mut *tx, book_uuid, &overrides, &json, false, user_id).await?;
    }
    touch_book_last_modified(&mut tx, book_uuid).await?;
    tx.commit().await?;

    // Only the "no override left at all" branch above stales the export-EPUB
    // cache — see [`super::delete_metadata_overrides`]'s equivalent cleanup
    // (#1395). A cover-only clear that leaves text overrides in place still
    // needs a rewrite (just without the cover swap), which the staleness
    // check in `rewritten_epub_path` already handles via the `last_modified`
    // bump.
    invalidate_export_epub_cache_for(cleared_book_id).await;
    Ok(())
}

/// Apply a `MetadataOverrides` to an `EbookMetadata`, mutating it in place,
/// gated by whether `precedence` (the owning scan root's configured
/// metadata-source order) ranks `OmnibusOverrides` above
/// `EmbeddedTags` — the two sources with a real data provider today (see
/// [`overrides_outrank_embedded`]). Scalar fields are replaced when `Some`;
/// m2m fields (`creators`, `subjects`, `genres`) replace entirely when
/// present. `genres` has no scanned counterpart — this is the only place a
/// book's genres are ever populated.
pub(crate) fn apply_overrides(
    book: &mut EbookMetadata,
    uuid: &str,
    ov: &MetadataOverrides,
    has_cover_override: bool,
    precedence: &[MetadataSource],
) {
    if !overrides_outrank_embedded(precedence) {
        return;
    }
    if let Some(ref t) = ov.title {
        book.title = Some(t.clone());
    }
    if let Some(ref d) = ov.description {
        book.description = crate::books::sanitize_description(Some(d.clone()));
    }
    if let Some(ref p) = ov.publisher {
        book.publisher = Some(p.clone());
    }
    if let Some(ref d) = ov.published {
        book.published = Some(d.clone());
    }
    if let Some(ref l) = ov.language {
        book.language = Some(l.clone());
    }
    if let Some(ref s) = ov.series {
        book.series = Some(s.clone());
    }
    if let Some(ref si) = ov.series_index {
        book.series_index = Some(si.clone());
    }
    if let Some(ref i) = ov.isbn13 {
        // Empty string is the clear sentinel (mirrors `build_overrides` on
        // the frontend) — an override that clears the field must read back
        // as "no ISBN", not as a literal empty string.
        book.isbn13 = if i.is_empty() { None } else { Some(i.clone()) };
    }
    if let Some(ref i) = ov.isbn10 {
        // Same empty-string-clears convention as `isbn13` above.
        book.isbn10 = if i.is_empty() { None } else { Some(i.clone()) };
    }
    if let Some(ref c) = ov.creators {
        book.creators = c.clone();
    }
    if let Some(ref s) = ov.subjects {
        book.subjects = s.clone();
    }
    if let Some(ref g) = ov.genres {
        book.genres = g.clone();
    }
    if let Some(p) = ov.print_pages {
        book.print_pages = Some(p);
    }
    book.has_cover_override = has_cover_override;
    if has_cover_override {
        // Ensure cover_url is set even if the original had no cover. The
        // REST route is uuid-keyed (`/api/covers/{uuid}`), matching the
        // non-override cover_url construction in books.rs — never `book.id`.
        book.cover_url = Some(format!("/api/covers/{uuid}"));
    }
    book.has_override = true;
}

/// Whether `MetadataSource::OmnibusOverrides` should win over
/// `MetadataSource::EmbeddedTags` (the scanned baseline) under a library's
/// configured precedence order. List order is lowest-to-highest priority,
/// so overrides win when they appear *after* embedded tags.
/// `FolderStructure`/`OpfSidecar`/`ProviderMatch` have no data provider yet
/// (see [`apply_overrides`]'s doc comment), so their position in the list
/// doesn't currently affect this.
///
/// A malformed/partial list missing one of the two real sources falls back
/// to the legacy always-wins behavior rather than silently dropping
/// overrides.
fn overrides_outrank_embedded(precedence: &[MetadataSource]) -> bool {
    let embedded = precedence
        .iter()
        .position(|s| *s == MetadataSource::EmbeddedTags);
    let overrides = precedence
        .iter()
        .position(|s| *s == MetadataSource::OmnibusOverrides);
    match (embedded, overrides) {
        (Some(e), Some(o)) => o > e,
        _ => true,
    }
}

/// Write a user-uploaded override cover to disk.
pub fn write_override_cover(
    uuid: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<(), MetadataOverridesError> {
    let ext = crate::covers::ImageFormat::from_mime(mime).to_ext();
    let dir = crate::covers::covers_dir();
    std::fs::create_dir_all(&dir)?;

    // Remove any existing override cover with a different extension. A
    // missing file is the expected/common case (most probed extensions
    // won't exist) and is ignored; any other failure (e.g. permissions)
    // must propagate — silently leaving a stale file behind would let the
    // extension probe in `find_override_cover_file` keep serving it instead
    // of the cover just written below.
    for fmt in crate::covers::ImageFormat::PROBE_ORDER {
        let old = dir.join(format!("override-{uuid}.{}", fmt.to_ext()));
        match std::fs::remove_file(old) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    std::fs::write(dir.join(format!("override-{uuid}.{ext}")), bytes)?;
    Ok(())
}

/// Delete override cover files for a UUID.
pub fn delete_override_cover(uuid: &str) {
    let dir = crate::covers::covers_dir();
    for fmt in crate::covers::ImageFormat::PROBE_ORDER {
        let _ = std::fs::remove_file(dir.join(format!("override-{uuid}.{}", fmt.to_ext())));
    }
}
