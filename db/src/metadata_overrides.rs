//! Metadata overrides: the user-editable layer on top of the canonical
//! scanned metadata. Persists `MetadataOverrides` JSON per `book_uuid`
//! and merges it on top of the read path.

use std::collections::HashMap;

use sqlx::{Executor, SqlitePool};

use omnibus_shared::{EbookMetadata, MetadataOverrides};

/// Errors returned by `get_book_uuid` and `rebuild_fts_for_book`. Other
/// public functions in this module still return `sqlx::Error` directly —
/// widening that is tracked separately.
#[derive(Debug, thiserror::Error)]
pub enum MetadataOverridesError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for MetadataOverridesError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => MetadataOverridesError::Db(inner),
        }
    }
}

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
    if let Err(e) = materialize_series_link(pool, book_uuid, overrides).await {
        tracing::warn!(book_uuid, error = %e, "series link materialize after override upsert failed");
    }
    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override upsert failed");
    }
    Ok(())
}

/// Merge `incoming` field overrides on top of any existing overrides for
/// `book_uuid` and persist the result inside one `BEGIN IMMEDIATE`
/// transaction, so two concurrent edits to the same book can't interleave
/// and silently drop each other's changes. The existing `has_cover_override`
/// flag is carried forward — a text-only edit must not clear a cover the
/// user uploaded earlier. The `books_fts` rebuild runs best-effort after
/// commit.
pub async fn merge_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
    incoming: &MetadataOverrides,
    user_id: i64,
) -> Result<(), sqlx::Error> {
    // `begin_with("BEGIN IMMEDIATE")` gives the same RESERVED-lock-at-start
    // semantics as the old hand-rolled statement (so concurrent edits to the
    // same book serialize instead of interleaving), but returns a real
    // `sqlx::Transaction`. Any `?` early-return below drops `tx` without a
    // commit, and the Drop impl issues a structured ROLLBACK — no reliance on
    // connection-drop implicit cleanup.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let existing: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(&mut *tx)
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
    upsert_overrides_row(&mut *tx, book_uuid, &json, has_cover_override, user_id).await?;
    tx.commit().await?;

    if let Err(e) = materialize_series_link(pool, book_uuid, &merged).await {
        tracing::warn!(book_uuid, error = %e, "series link materialize after override merge failed");
    }
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
pub async fn get_book_uuid(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<String>, MetadataOverridesError> {
    Ok(sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await?)
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
    uuid: &str,
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
        // Ensure cover_url is set even if the original had no cover. The
        // REST route is uuid-keyed (`/api/covers/{uuid}`), matching the
        // non-override cover_url construction in books.rs — never `book.id`.
        book.cover_url = Some(format!("/api/covers/{uuid}"));
    }
    book.has_override = true;
}

/// When an override sets a series name, ensure a `series` row and
/// `books_series_link` exist so the browse index and detail-page breadcrumb
/// resolve. Without this, override-only series are invisible to
/// `list_series` (which requires a canonical link for visibility) and the
/// book detail page's `series_id` backfill can't find the `series.id`.
async fn materialize_series_link(
    pool: &SqlitePool,
    book_uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<(), sqlx::Error> {
    let series_name = overrides
        .series
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(series_name) = series_name else {
        return Ok(());
    };
    let Some(book_id) = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(book_uuid)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(());
    };

    sqlx::query("INSERT OR IGNORE INTO series (name) VALUES (?)")
        .bind(series_name)
        .execute(pool)
        .await?;

    let series_id =
        sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ? COLLATE NOCASE")
            .bind(series_name)
            .fetch_one(pool)
            .await?;

    sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
        .bind(book_id)
        .bind(series_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Rebuild the `books_fts` row for the book identified by `book_uuid` using
/// the merged metadata returned from [`crate::books::get_book`] (canonical
/// taxonomy with overrides applied). Called from the override write paths
/// so search matches what the UI displays.
///
/// Silently returns `Ok(())` if the UUID has no matching book — overrides
/// for an unknown UUID would only happen if a book row was deleted out from
/// under us, in which case there is no FTS row to maintain.
pub(crate) async fn rebuild_fts_for_book(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), MetadataOverridesError> {
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

/// Rebuild `books_fts` rows for all books identified by `book_uuids` using at
/// most **2 SQL write statements per chunk** regardless of how many UUIDs are
/// supplied: one batched `DELETE … WHERE rowid IN (…)` then one multi-row
/// `INSERT`. Chunks at 100 UUIDs to stay well under SQLite's 999 bind-
/// parameter cap (each FTS row uses 7 params, so 100 rows = 700 params).
///
/// The per-book merged metadata is fetched individually (N reads) before the
/// writes so that overrides are applied correctly, but those reads do not hold
/// the write lock.
///
/// UUIDs with no matching `books` row are silently skipped. Returns
/// immediately without opening a write transaction if the resolved list is
/// empty.
pub(crate) async fn rebuild_fts_for_books_batch(
    pool: &SqlitePool,
    book_uuids: &[String],
) -> Result<(), MetadataOverridesError> {
    if book_uuids.is_empty() {
        return Ok(());
    }

    // Phase 1 (reads): resolve each UUID to a book_id and fetch its merged
    // metadata. Skips any UUID that no longer maps to a live book row.
    struct FtsRow {
        book_id: i64,
        title: String,
        authors: String,
        series: String,
        tags: String,
        description: String,
        isbn: String,
    }

    let mut rows: Vec<FtsRow> = Vec::with_capacity(book_uuids.len());
    for uuid in book_uuids {
        let Some(book_id) = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
            .bind(uuid)
            .fetch_optional(pool)
            .await?
        else {
            continue;
        };
        let Some(merged) = crate::books::get_book(pool, book_id).await? else {
            continue;
        };

        let title = merged.title.clone().unwrap_or_default();
        let isbn = merged
            .identifiers
            .iter()
            .find(|i| {
                i.scheme
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("ISBN"))
            })
            .map(|i| i.value.clone())
            .unwrap_or_default();
        let authors = crate::helpers::join_names(merged.creators.iter().map(|c| c.name.as_str()));
        let series = merged.series.clone().unwrap_or_default();
        let tags = crate::helpers::join_names(merged.subjects.iter().map(String::as_str));
        let description = merged.description.clone().unwrap_or_default();

        rows.push(FtsRow {
            book_id,
            title,
            authors,
            series,
            tags,
            description,
            isbn,
        });
    }

    if rows.is_empty() {
        return Ok(());
    }

    // Phase 2 (writes): one DELETE + one multi-row INSERT per chunk of 100.
    // 100 rows × 7 params = 700 bind parameters, safely under the 999 cap.
    let mut tx = pool.begin().await?;
    for chunk in rows.chunks(100) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        // One batched DELETE for the whole chunk.
        let delete_sql = format!("DELETE FROM books_fts WHERE rowid IN ({placeholders})");
        let mut q = sqlx::query(&delete_sql);
        for row in chunk {
            q = q.bind(row.book_id);
        }
        q.execute(&mut *tx).await?;

        // One multi-row INSERT for the whole chunk.
        let value_placeholders = std::iter::repeat_n("(?, ?, ?, ?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let insert_sql = format!(
            "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn) \
             VALUES {value_placeholders}"
        );
        let mut q = sqlx::query(&insert_sql);
        for row in chunk {
            q = q
                .bind(row.book_id)
                .bind(&row.title)
                .bind(&row.authors)
                .bind(&row.series)
                .bind(&row.tags)
                .bind(&row.description)
                .bind(&row.isbn);
        }
        q.execute(&mut *tx).await?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::{get_book, list_books, search_books};
    use crate::palette::search_palette;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::test_support::{indexed, CoversTempDir};
    use omnibus_shared::MetadataOverrides;

    // -----------------------------------------------------------------
    // F5.1 Metadata overrides
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn upsert_and_get_metadata_overrides_roundtrips() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        // Create a user for updated_by.
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let ov = MetadataOverrides {
            title: Some("New Title".into()),
            description: Some("A new description".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "test-uuid-1", &ov, false, user_id)
            .await
            .unwrap();

        let (loaded, has_cover) = get_metadata_overrides(&pool, "test-uuid-1")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(loaded.title, Some("New Title".into()));
        assert_eq!(loaded.description, Some("A new description".into()));
        assert_eq!(loaded.publisher, None);
        assert!(!has_cover);
    }
    #[tokio::test]
    async fn merge_metadata_overrides_accumulates_fields_and_preserves_cover_flag() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Seed an existing override carrying a title AND a user-uploaded cover.
        let initial = MetadataOverrides {
            title: Some("First Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "merge-uuid", &initial, true, user_id)
            .await
            .unwrap();

        // A later edit touching only `description` must not clobber the title
        // (the incremental-edit contract the TOCTOU race nullified) and must
        // not reset the cover flag (the pre-#166 reset bug).
        let edit = MetadataOverrides {
            description: Some("Added later".into()),
            ..Default::default()
        };
        merge_metadata_overrides(&pool, "merge-uuid", &edit, user_id)
            .await
            .unwrap();

        let (loaded, has_cover) = get_metadata_overrides(&pool, "merge-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(
            loaded.title,
            Some("First Title".into()),
            "prior title must survive a description-only merge"
        );
        assert_eq!(loaded.description, Some("Added later".into()));
        assert!(
            has_cover,
            "has_cover_override must carry forward across a text-only merge"
        );
    }
    #[tokio::test]
    async fn merge_metadata_overrides_creates_row_when_absent() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let edit = MetadataOverrides {
            title: Some("Fresh".into()),
            ..Default::default()
        };
        merge_metadata_overrides(&pool, "fresh-uuid", &edit, user_id)
            .await
            .unwrap();
        let (loaded, has_cover) = get_metadata_overrides(&pool, "fresh-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(loaded.title, Some("Fresh".into()));
        assert!(!has_cover, "a brand-new merged row has no cover override");
    }
    /// #243: two concurrent saves to the same book (e.g. the F5.1 edit form
    /// open in two tabs, or a network retry firing twice) each touch a
    /// different field. Because the rpc/REST save paths route through
    /// `merge_metadata_overrides` — whose read-merge-write runs under a single
    /// `BEGIN IMMEDIATE` — neither write may be silently dropped: both fields
    /// must survive regardless of interleaving. A barrier releases both tasks
    /// into the merge at the same instant so the test exercises real contention
    /// rather than letting the first save finish before the second starts.
    #[tokio::test]
    async fn merge_metadata_overrides_concurrent_saves_dont_drop_writes() {
        use std::sync::Arc;
        use tokio::sync::Barrier;

        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let barrier = Arc::new(Barrier::new(2));
        let pool_a = pool.clone();
        let pool_b = pool.clone();
        let barrier_a = barrier.clone();
        let barrier_b = barrier.clone();
        let save_title = tokio::spawn(async move {
            barrier_a.wait().await;
            merge_metadata_overrides(
                &pool_a,
                "race-uuid",
                &MetadataOverrides {
                    title: Some("Title From Tab A".into()),
                    ..Default::default()
                },
                user_id,
            )
            .await
        });
        let save_publisher = tokio::spawn(async move {
            barrier_b.wait().await;
            merge_metadata_overrides(
                &pool_b,
                "race-uuid",
                &MetadataOverrides {
                    publisher: Some("Publisher From Tab B".into()),
                    ..Default::default()
                },
                user_id,
            )
            .await
        });

        save_title.await.unwrap().unwrap();
        save_publisher.await.unwrap().unwrap();

        let (loaded, _) = get_metadata_overrides(&pool, "race-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(
            loaded.title,
            Some("Title From Tab A".into()),
            "tab A's title must not be lost to tab B's concurrent save"
        );
        assert_eq!(
            loaded.publisher,
            Some("Publisher From Tab B".into()),
            "tab B's publisher must not be lost to tab A's concurrent save"
        );
    }
    #[tokio::test]
    async fn get_metadata_overrides_returns_none_when_absent() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let result = get_metadata_overrides(&pool, "nonexistent-uuid")
            .await
            .unwrap();
        assert!(result.is_none());
    }
    #[tokio::test]
    async fn delete_metadata_overrides_removes_row() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ov = MetadataOverrides {
            title: Some("Override".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "del-uuid", &ov, false, user_id)
            .await
            .unwrap();
        assert!(get_metadata_overrides(&pool, "del-uuid")
            .await
            .unwrap()
            .is_some());

        delete_metadata_overrides(&pool, "del-uuid").await.unwrap();
        assert!(get_metadata_overrides(&pool, "del-uuid")
            .await
            .unwrap()
            .is_none());
    }
    /// Bug #1: saving a title override must rebuild `books_fts` so search
    /// finds the new title and stops matching the original one.
    #[tokio::test]
    async fn upsert_metadata_overrides_rebuilds_fts_for_title() {
        let _covers = CoversTempDir::new("fts_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Original Title"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Sanity: search finds the original title.
        let hits = search_books(&pool, "/lib", "Original").await.unwrap();
        assert_eq!(hits.len(), 1);

        // Save an override that changes the title.
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        let ov = MetadataOverrides {
            title: Some("Brand New Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Search now matches the overridden title and no longer the original.
        let new_hits = search_books(&pool, "/lib", "Brand").await.unwrap();
        assert_eq!(new_hits.len(), 1);
        assert_eq!(new_hits[0].title.as_deref(), Some("Brand New Title"));
        let old_hits = search_books(&pool, "/lib", "Original").await.unwrap();
        assert!(
            old_hits.is_empty(),
            "FTS still matches the pre-override title"
        );
    }
    /// Bug #1: the palette uses the same `books_fts` table, so the override
    /// rebuild must also surface there.
    #[tokio::test]
    async fn upsert_metadata_overrides_rebuilds_fts_for_palette() {
        let _covers = CoversTempDir::new("fts_override_palette");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        let ov = MetadataOverrides {
            title: Some("Edited Palette Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
    }
    /// Bug #1 follow-on: deleting the override should restore the FTS row
    /// to the canonical scanned values.
    #[tokio::test]
    async fn delete_metadata_overrides_restores_fts() {
        let _covers = CoversTempDir::new("fts_override_revert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "r.epub",
                Some("Canonical Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();

        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Temporary Override".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        assert_eq!(
            search_books(&pool, "/lib", "Temporary")
                .await
                .unwrap()
                .len(),
            1
        );

        delete_metadata_overrides(&pool, &uuid).await.unwrap();

        // FTS is back to the canonical title; the override token no longer
        // matches.
        assert_eq!(
            search_books(&pool, "/lib", "Canonical")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(search_books(&pool, "/lib", "Temporary")
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn delete_overrides_reverts_to_scanned() {
        let _covers = CoversTempDir::new("revert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "revert.epub",
                Some("Original"),
                &["Author"],
                &["fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        // Override.
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Changed".into()),
                subjects: Some(vec!["sci-fi".into()]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.title.as_deref(), Some("Changed"));

        // Delete overrides — should revert to scanned.
        delete_metadata_overrides(&pool, &uuid).await.unwrap();
        let reverted = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(reverted.title.as_deref(), Some("Original"));
        assert_eq!(reverted.subjects, vec!["fiction"]);
        assert!(!reverted.has_override);
    }
    /// Verify that `MetadataOverrides::merge` correctly layers a second edit
    /// on top of a first without losing the first edit's fields.
    #[tokio::test]
    async fn merge_preserves_prior_overrides() {
        let first = MetadataOverrides {
            title: Some("Edited Title".into()),
            publisher: Some("Edited Publisher".into()),
            ..Default::default()
        };
        let second = MetadataOverrides {
            description: Some("New description".into()),
            ..Default::default()
        };
        let merged = first.merge(&second);
        // second's description wins
        assert_eq!(merged.description.as_deref(), Some("New description"));
        // first's title and publisher are preserved (not wiped by None)
        assert_eq!(merged.title.as_deref(), Some("Edited Title"));
        assert_eq!(merged.publisher.as_deref(), Some("Edited Publisher"));
        // unset in both stays None
        assert_eq!(merged.language, None);
    }
    #[tokio::test]
    async fn upsert_overrides_materializes_series_link_for_new_series() {
        let _covers = CoversTempDir::new("materialize_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        crate::sync::sync_audiobooks(
            &pool,
            "/audio",
            crate::sync::AudiobookSyncPlan {
                new_books: vec![crate::test_support::indexed_audiobook(
                    "author/book",
                    "My Audiobook",
                    Some("Narrator"),
                )],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let book = list_books(&pool, "/audio").await.unwrap();
        let uuid = book[0].unique_identifier.clone().unwrap();
        let id = book[0].id;

        assert!(
            get_book(&pool, id)
                .await
                .unwrap()
                .unwrap()
                .series_id
                .is_none(),
            "no series before override"
        );

        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                series: Some("My Series".into()),
                series_index: Some("1".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let book = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(book.series.as_deref(), Some("My Series"));
        assert!(
            book.series_id.is_some(),
            "series_id should be set after override materializes the link"
        );
    }
}
