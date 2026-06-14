//! The JSON snapshot of a merge's source book, stored in
//! `merge_log.source_metadata` and replayed by `undo_merge`.

use serde::{Deserialize, Serialize};
use sqlx::Transaction;

/// Everything needed to recreate the absorbed `books` row and its
/// satellite rows on undo. Link rows are stored **by name** — the
/// taxonomy ids may be garbage-collected between merge and undo.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SourceSnapshot {
    pub uuid: String,
    /// The source book's `scan_roots.path` (re-resolved or recreated on
    /// undo) — also the `merged_uuids.library_path` for the guard row.
    pub library_path: String,
    pub path: String,
    pub title: String,
    pub sort: Option<String>,
    pub author_sort: Option<String>,
    pub series_index: Option<f64>,
    pub pubdate: Option<String>,
    pub timestamp: Option<String>,
    pub has_cover: i64,
    pub description: Option<String>,
    pub accent_color: Option<String>,
    pub title_norm: Option<String>,
    pub author_norm: Option<String>,
    /// `book_files.format`s re-parented onto the target — undo moves
    /// exactly these back.
    pub moved_formats: Vec<String>,
    /// `book_files.id`s re-parented onto the target — undo moves exactly
    /// these rows back (format alone is ambiguous when same-format merges
    /// are allowed).
    #[serde(default)]
    pub moved_file_ids: Vec<i64>,
    /// `moved_formats` minus the formats that were themselves
    /// attachments (present in the source's own `merged_uuids` rows).
    /// The source-uuid reindex guard is recorded per native format.
    pub native_formats: Vec<String>,
    /// `(name, sort, position)`.
    pub authors: Vec<(String, Option<String>, i64)>,
    pub series: Vec<String>,
    pub tags: Vec<String>,
    pub publishers: Vec<String>,
    pub languages: Vec<String>,
    /// `(scheme, value)`.
    pub identifiers: Vec<(String, String)>,
    /// `merged_uuids` rows that pointed at the source pre-merge:
    /// `(uuid, format, library_path)`. Re-pointed to the target by the
    /// merge; pointed back at the recreated source by undo.
    pub merged_uuid_rows: Vec<(String, String, String)>,
}

/// Load the full snapshot for `book_id` inside the merge transaction.
pub(super) async fn build_snapshot(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
) -> Result<SourceSnapshot, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let (
        uuid,
        library_path,
        path,
        title,
        sort,
        author_sort,
        series_index,
        pubdate,
        timestamp,
        has_cover,
        description,
        accent_color,
        title_norm,
        author_norm,
    ): (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT b.uuid, l.path, b.path, b.title, b.sort, b.author_sort, b.series_index,
                b.pubdate, b.timestamp, b.has_cover, b.description, b.accent_color,
                b.title_norm, b.author_norm
           FROM books b JOIN scan_roots l ON l.id = b.library_id
          WHERE b.id = ?",
    )
    .bind(book_id)
    .fetch_one(&mut **tx)
    .await?;

    let moved_formats: Vec<String> =
        sqlx::query_scalar("SELECT format FROM book_files WHERE book_id = ? ORDER BY format")
            .bind(book_id)
            .fetch_all(&mut **tx)
            .await?;
    let moved_file_ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? ORDER BY id")
            .bind(book_id)
            .fetch_all(&mut **tx)
            .await?;
    let merged_uuid_rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT uuid, format, library_path FROM merged_uuids WHERE book_id = ? ORDER BY uuid",
    )
    .bind(book_id)
    .fetch_all(&mut **tx)
    .await?;
    let attached_formats: Vec<String> = merged_uuid_rows
        .iter()
        .map(|(_, f, _)| f.to_uppercase())
        .collect();
    let native_formats: Vec<String> = moved_formats
        .iter()
        .filter(|f| !attached_formats.contains(&f.to_uppercase()))
        .cloned()
        .collect();

    let authors: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT a.name, a.sort, l.position FROM books_authors_link l
           JOIN authors a ON a.id = l.author WHERE l.book = ? ORDER BY l.position",
    )
    .bind(book_id)
    .fetch_all(&mut **tx)
    .await?;
    let series: Vec<String> = sqlx::query_scalar(
        "SELECT s.name FROM books_series_link l JOIN series s ON s.id = l.series WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&mut **tx)
    .await?;
    let tags: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM books_tags_link l JOIN tags t ON t.id = l.tag WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&mut **tx)
    .await?;
    let publishers: Vec<String> = sqlx::query_scalar(
        "SELECT p.name FROM books_publishers_link l
           JOIN publishers p ON p.id = l.publisher WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&mut **tx)
    .await?;
    let languages: Vec<String> = sqlx::query_scalar(
        "SELECT g.code FROM books_languages_link l
           JOIN languages g ON g.id = l.language WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&mut **tx)
    .await?;
    let identifiers: Vec<(String, String)> =
        sqlx::query_as("SELECT scheme, value FROM book_identifiers WHERE book_id = ?")
            .bind(book_id)
            .fetch_all(&mut **tx)
            .await?;

    Ok(SourceSnapshot {
        uuid,
        library_path,
        path,
        title,
        sort,
        author_sort,
        series_index,
        pubdate,
        timestamp,
        has_cover,
        description,
        accent_color,
        title_norm,
        author_norm,
        moved_formats,
        moved_file_ids,
        native_formats,
        authors,
        series,
        tags,
        publishers,
        languages,
        identifiers,
        merged_uuid_rows,
    })
}
