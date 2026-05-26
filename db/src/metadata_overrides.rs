//! F5.1 metadata overrides: the user-editable layer on top of the
//! canonical scanned metadata. Persists `MetadataOverrides` JSON per
//! `book_uuid` and merges it on top of the read path.

use std::collections::HashMap;

use sqlx::{Executor, SqlitePool};

use omnibus_shared::{EbookMetadata, MetadataOverrides};

/// Execute the `metadata_overrides` INSERT…ON CONFLICT against any executor —
/// a `&SqlitePool` for the fire-and-forget [`upsert_metadata_overrides`] path,
/// or a transaction connection for the serialized [`merge_metadata_overrides`]
/// path — so the upsert statement lives in exactly one place and the two
/// callers can't drift (e.g. when a column or default changes).
async fn upsert_overrides_row<'e, E>(
    executor: E,
    book_uuid: &str,
    overrides_json: &str,
    has_cover_override: bool,
    user_id: i64,
) -> Result<(), sqlx::Error>
where
    E: Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, has_cover_override, updated_by, updated_at)
         VALUES (?, ?, ?, ?, datetime('now'))
         ON CONFLICT(book_uuid) DO UPDATE SET
           overrides = excluded.overrides,
           has_cover_override = excluded.has_cover_override,
           updated_by = excluded.updated_by,
           updated_at = datetime('now')",
    )
    .bind(book_uuid)
    .bind(overrides_json)
    .bind(i64::from(has_cover_override))
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Upsert user metadata overrides for a book identified by its stable UUID.
/// The `overrides` are JSON-serialized into the `metadata_overrides` table.
///
/// After the upsert, the book's `books_fts` row is rebuilt from the merged
/// (canonical + override) metadata so that search results stay consistent
/// with what the UI displays. The FTS rebuild is best-effort: rebuild
/// failures are logged but do not fail the override save, since the
/// override write is the user's actual intent.
pub async fn upsert_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
    overrides: &MetadataOverrides,
    has_cover_override: bool,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_string(overrides).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    upsert_overrides_row(pool, book_uuid, &json, has_cover_override, user_id).await?;
    // Best-effort FTS rebuild: log and continue on failure. The override
    // write is the user's actual intent — a stale FTS row gets fixed on the
    // next reindex, but surfacing a 500 here would make them think their
    // save was lost. Matches the docstring contract above.
    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override upsert failed");
    }
    Ok(())
}

/// Merge `incoming` field overrides on top of any existing overrides for
/// `book_uuid` and persist the result inside one `BEGIN IMMEDIATE`
/// transaction, so two concurrent edits to the same book can't interleave and
/// silently drop each other's changes (#166). The existing `has_cover_override`
/// flag is carried forward — a text-only edit must not clear a cover the user
/// uploaded earlier. The `books_fts` rebuild runs best-effort after commit.
pub async fn merge_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
    incoming: &MetadataOverrides,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    // Mirror `auth::create_user`'s explicit transaction idiom: async drop
    // isn't stable, so we acquire one connection, run BEGIN IMMEDIATE, and
    // COMMIT/ROLLBACK by hand based on the inner result.
    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    let result: Result<(), sqlx::Error> = async {
        let existing: Option<(String, i64)> = sqlx::query_as(
            "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
        )
        .bind(book_uuid)
        .fetch_optional(&mut *conn)
        .await?;

        let (merged, has_cover_override) = match existing {
            Some((json, has_cover)) => {
                let prior: MetadataOverrides =
                    serde_json::from_str(&json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
                (prior.merge(incoming), has_cover != 0)
            }
            None => (incoming.clone(), false),
        };

        let json = serde_json::to_string(&merged).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        upsert_overrides_row(&mut *conn, book_uuid, &json, has_cover_override, user_id).await?;
        Ok(())
    }
    .await;

    match &result {
        Ok(()) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
        }
        Err(_) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        }
    }
    result?;

    // Best-effort FTS rebuild — same rationale as `upsert_metadata_overrides`.
    // Kept outside the transaction so the write lock is released first.
    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override merge failed");
    }
    Ok(())
}

/// Load overrides for a single book UUID. Returns `None` if no overrides
/// exist.
pub async fn get_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Option<(MetadataOverrides, bool)>, sqlx::Error> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((json, has_cover)) => {
            let ov: MetadataOverrides =
                serde_json::from_str(&json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
            Ok(Some((ov, has_cover != 0)))
        }
        None => Ok(None),
    }
}

/// Delete overrides for a book UUID (revert to scanned values).
///
/// Also rebuilds the book's `books_fts` row so that search reverts to
/// matching the canonical scanned metadata, mirroring
/// [`upsert_metadata_overrides`].
pub async fn delete_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM metadata_overrides WHERE book_uuid = ?")
        .bind(book_uuid)
        .execute(pool)
        .await?;
    // Best-effort FTS rebuild — same rationale as `upsert_metadata_overrides`.
    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override delete failed");
    }
    Ok(())
}

/// Look up the UUID for a given `books.id`. Used by the override-save
/// endpoints to bridge the id-based API with the uuid-keyed overrides table.
pub async fn get_book_uuid(pool: &SqlitePool, book_id: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await
}

/// Bulk-load overrides for a set of UUIDs. Returns a map from UUID to
/// `(overrides, has_cover_override)`. Used by `list_books` and `search_books`
/// to merge overrides without N+1 queries.
pub(crate) async fn load_overrides_bulk(
    pool: &SqlitePool,
    uuids: &[String],
) -> Result<HashMap<String, (MetadataOverrides, bool)>, sqlx::Error> {
    if uuids.is_empty() {
        return Ok(HashMap::new());
    }

    // SQLite has a limit on bound parameters (999 by default). For very large
    // libraries we chunk, but typical libraries have < 10k books.
    let mut map = HashMap::with_capacity(uuids.len());

    for chunk in uuids.chunks(500) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT book_uuid, overrides, has_cover_override FROM metadata_overrides WHERE book_uuid IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, String, i64)>(&sql);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let rows = q.fetch_all(pool).await?;
        for (uuid, json, has_cover) in rows {
            match serde_json::from_str::<MetadataOverrides>(&json) {
                Ok(ov) => {
                    map.insert(uuid, (ov, has_cover != 0));
                }
                Err(e) => {
                    tracing::warn!(
                        book_uuid = %uuid,
                        error = %e,
                        "corrupt metadata_overrides JSON — skipping row"
                    );
                }
            }
        }
    }

    Ok(map)
}

/// Apply a `MetadataOverrides` to an `EbookMetadata`, mutating it in place.
/// Scalar fields are replaced when `Some`; m2m fields (`creators`, `subjects`)
/// replace entirely when present.
pub(crate) fn apply_overrides(
    book: &mut EbookMetadata,
    ov: &MetadataOverrides,
    has_cover_override: bool,
) {
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
    if let Some(ref c) = ov.creators {
        book.creators = c.clone();
    }
    if let Some(ref s) = ov.subjects {
        book.subjects = s.clone();
    }
    if has_cover_override {
        // Ensure cover_url is set even if the original had no cover.
        book.cover_url = Some(format!("/api/covers/{}", book.id));
    }
    book.has_override = true;
}

/// Rebuild the `books_fts` row for the book identified by `book_uuid` using
/// the merged metadata returned from [`crate::queries::get_book`] (canonical
/// taxonomy with overrides applied). Called from the override write paths
/// so search matches what the UI displays.
///
/// Silently returns `Ok(())` if the UUID has no matching book — overrides
/// for an unknown UUID would only happen if a book row was deleted out from
/// under us, in which case there is no FTS row to maintain.
pub(crate) async fn rebuild_fts_for_book(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), sqlx::Error> {
    let Some(book_id) = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(book_uuid)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(());
    };

    let Some(merged) = crate::books::get_book(pool, book_id).await? else {
        return Ok(());
    };
    let title = merged.title.clone().unwrap_or_default();
    let first_isbn = merged
        .identifiers
        .iter()
        .find(|i| {
            i.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("ISBN"))
        })
        .map(|i| i.value.clone());

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .execute(&mut *tx)
        .await?;
    crate::sync::insert_fts_row(&mut tx, book_id, &title, first_isbn.as_deref(), &merged).await?;
    tx.commit().await?;
    Ok(())
}

/// Write a user-uploaded override cover to disk.
pub fn write_override_cover(uuid: &str, mime: &str, bytes: &[u8]) -> std::io::Result<()> {
    let ext = crate::covers::ImageFormat::from_mime(mime).to_ext();
    let dir = crate::covers::covers_dir();
    std::fs::create_dir_all(&dir)?;

    // Remove any existing override cover with a different extension.
    for fmt in crate::covers::ImageFormat::PROBE_ORDER {
        let old = dir.join(format!("override-{uuid}.{}", fmt.to_ext()));
        let _ = std::fs::remove_file(old);
    }

    std::fs::write(dir.join(format!("override-{uuid}.{ext}")), bytes)
}

/// Delete override cover files for a UUID.
pub fn delete_override_cover(uuid: &str) {
    let dir = crate::covers::covers_dir();
    for fmt in crate::covers::ImageFormat::PROBE_ORDER {
        let _ = std::fs::remove_file(dir.join(format!("override-{uuid}.{}", fmt.to_ext())));
    }
}
