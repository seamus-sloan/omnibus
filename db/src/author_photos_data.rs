//! Author profile photo data layer plus the admin `delete_author`
//! primitive. `author_photos` holds at most one row per author (PK =
//! author_id) with a `source` of `'manual'`, `'openlibrary'`, or
//! `'letter'` (negative-cache marker — NULL bytes/mime so `get_author_photo`
//! returns `None` and the letter avatar stays in place). Admin DELETE
//! clears the row to force re-resolution. The OL resolver itself lives in
//! [`crate::author_photos`]; this file owns just the row CRUD.

use sqlx::SqlitePool;

use crate::metadata_overrides::rebuild_fts_for_book;

/// Source-of-truth marker for a cached author photo row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorPhotoSource {
    Manual,
    OpenLibrary,
    Letter,
}

impl AuthorPhotoSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorPhotoSource::Manual => "manual",
            AuthorPhotoSource::OpenLibrary => "openlibrary",
            AuthorPhotoSource::Letter => "letter",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(Self::Manual),
            "openlibrary" => Some(Self::OpenLibrary),
            "letter" => Some(Self::Letter),
            _ => None,
        }
    }
}

/// Fetch a cached profile photo for the given author. Returns `None` when no
/// row exists or the row is a `'letter'` negative-cache marker — both cases
/// should produce a 404 from the GET handler so the frontend keeps rendering
/// the letter avatar.
pub async fn get_author_photo(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    // Filter `letter` rows in SQL as well as via the schema CHECK constraint
    // so a malformed row (e.g. left over from a future migration drift)
    // can never accidentally serve `letter` bytes as a real image. Belt +
    // suspenders alongside the table-level invariant.
    let row: Option<(String, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT source, mime, bytes
           FROM author_photos
          WHERE author_id = ? AND source <> 'letter'",
    )
    .bind(author_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((_, Some(mime), Some(bytes))) if !bytes.is_empty() => Ok(Some((mime, bytes))),
        _ => Ok(None),
    }
}

/// Look up just the cascade-state metadata (source + fetched_at) for an
/// author. Used by the resolver to decide whether to skip resolution — a
/// `'letter'` row prevents re-querying Open Library until an admin clears it
/// via `delete_author_photo`.
pub async fn author_photo_status(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<(AuthorPhotoSource, String)>, sqlx::Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source, fetched_at FROM author_photos WHERE author_id = ?")
            .bind(author_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(s, t)| AuthorPhotoSource::parse(&s).map(|src| (src, t))))
}

/// Upsert a photo row. Replaces any existing row (PRIMARY KEY conflict on
/// `author_id`). `bytes` / `mime` are `None` for `'letter'` negative-cache
/// markers; `url` is `None` for manual uploads.
pub async fn upsert_author_photo(
    pool: &SqlitePool,
    author_id: i64,
    source: AuthorPhotoSource,
    url: Option<&str>,
    mime: Option<&str>,
    bytes: Option<&[u8]>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, url, mime, bytes, fetched_at)
              VALUES (?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(author_id) DO UPDATE SET
              source     = excluded.source,
              url        = excluded.url,
              mime       = excluded.mime,
              bytes      = excluded.bytes,
              fetched_at = excluded.fetched_at",
    )
    .bind(author_id)
    .bind(source.as_str())
    .bind(url)
    .bind(mime)
    .bind(bytes)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop the cache row for an author so the next page view re-queues
/// resolution. Used by admin DELETE.
pub async fn delete_author_photo(pool: &SqlitePool, author_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM author_photos WHERE author_id = ?")
        .bind(author_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete an author by id and prevent reindex from re-creating the row.
///
/// One transaction: look up the name (for the blocklist insert) and
/// affected book ids (for the post-commit FTS refresh), drop the link
/// rows, drop the `authors` row, then `INSERT OR IGNORE` the name into
/// `ignored_authors` so the next `indexer::reindex` does not silently
/// re-create the row via `resolve_or_insert_author`.
///
/// Returns the number of books that were un-linked (used by the admin
/// confirmation modal to show "this affects N books"). Returns `Ok(0)`
/// without touching the blocklist if `author_id` does not exist — a
/// stale tab firing a second Delete must not leak a ghost row into
/// `ignored_authors`.
///
/// FTS refresh runs best-effort *after* commit, mirroring
/// [`crate::metadata_overrides::upsert_metadata_overrides`]: a stale FTS
/// row is fixed on the next reindex, but a refresh failure must not undo
/// the admin's intent.
pub async fn delete_author(pool: &SqlitePool, author_id: i64) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let Some(name): Option<String> = sqlx::query_scalar("SELECT name FROM authors WHERE id = ?")
        .bind(author_id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(0);
    };

    // Snapshot the affected book UUIDs *inside* the transaction by
    // joining link → books. Doing the lookup as a uuid join (one bind
    // parameter) instead of post-commit `WHERE id IN (?, ?, …)` keeps
    // the post-commit FTS refresh from hitting SQLite's 999 bind-
    // parameter cap on authors linked to >999 books — without that
    // cap a giant-author delete would silently skip its FTS rebuild
    // and leave the index stale.
    let affected_uuids: Vec<String> = sqlx::query_scalar(
        "SELECT b.uuid FROM books b
         JOIN books_authors_link l ON l.book = b.id
         WHERE l.author = ?",
    )
    .bind(author_id)
    .fetch_all(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM books_authors_link WHERE author = ?")
        .bind(author_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(author_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("INSERT OR IGNORE INTO ignored_authors(name) VALUES (?)")
        .bind(&name)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    for uuid in &affected_uuids {
        if let Err(e) = rebuild_fts_for_book(pool, uuid).await {
            tracing::warn!(uuid = uuid.as_str(), error = %e, "books_fts rebuild after delete_author failed");
        }
    }

    Ok(affected_uuids.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::list_books;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::test_support::*;

    // -------------------------------------------------------------------------
    // F1.11 author profile photo tests
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn author_photo_roundtrips_manual_upload() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let bytes = b"\xFF\xD8\xFFfake-jpeg".to_vec();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(&bytes),
        )
        .await
        .unwrap();

        let (mime, fetched) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(fetched, bytes);
    }
    #[tokio::test]
    async fn author_photo_letter_marker_returns_none() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();

        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }
    #[tokio::test]
    async fn author_photo_status_none_when_unset() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;
        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn author_photo_upsert_replaces_existing_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // Letter marker first, then a manual upload replaces it.
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/png"),
            Some(b"\x89PNG\r\n\x1a\nfake"),
        )
        .await
        .unwrap();

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Manual);
        let (mime, _) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/png");
    }
    #[tokio::test]
    async fn author_photo_delete_clears_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfoo"),
        )
        .await
        .unwrap();
        delete_author_photo(&pool, ada_id).await.unwrap();

        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
    }
    // -------------------------------------------------------------------------
    // ignored_authors blocklist (F5.9-lite reindex-resurrection guard)
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // delete_author (F5.9-lite admin "Delete author" primitive)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn delete_author_removes_links_and_inserts_blocklist_row() {
        let _covers = CoversTempDir::new("delete_author_basic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Junk Author"], &[], None, None),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["Junk Author", "Real Author"],
                    &[],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind("Junk Author")
            .fetch_one(&pool)
            .await
            .unwrap();

        let unlinked = delete_author(&pool, junk_id).await.unwrap();
        assert_eq!(unlinked, 2, "both books should report as un-linked");

        let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE id = ?")
            .bind(junk_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(junk_count, 0, "authors row should be gone");

        let link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE author = ?")
                .bind(junk_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            link_count, 0,
            "no link rows should remain for deleted author"
        );

        let blocklist_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
                .bind("Junk Author")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(blocklist_count, 1, "name must be added to ignored_authors");

        // Real Author on book B should still be linked.
        let books = list_books(&pool, "/lib").await.unwrap();
        let b = books
            .iter()
            .find(|x| x.title.as_deref() == Some("B"))
            .unwrap();
        let creators: Vec<&str> = b.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }
    #[tokio::test]
    async fn delete_author_is_no_op_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let unlinked = delete_author(&pool, 99_999).await.unwrap();
        assert_eq!(unlinked, 0);
        let blocklist_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            blocklist_count, 0,
            "missing id must not leak a ghost blocklist row"
        );
    }
    #[tokio::test]
    async fn delete_author_survives_reindex() {
        // Full durability check: delete the junk author, then run the
        // reindex pipeline against a fixture that *still* lists the junk
        // contributor in its OPF (simulated via replace_books with the
        // same input). The blocklist row inserted by delete_author must
        // keep resolve_or_insert_author from re-creating the author.
        let _covers = CoversTempDir::new("delete_author_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();

        let junk_name = "calibre (8.0.0) [https://calibre-ebook.com]";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        delete_author(&pool, junk_id).await.unwrap();

        // Simulated reindex: same OPF contents, run through the
        // indexing pipeline again. Without the blocklist guard this
        // would re-create the junk author and relink it.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            junk_after, 0,
            "second reindex must not resurrect a deleted author"
        );

        let books = list_books(&pool, "/lib").await.unwrap();
        let creators: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }
}
