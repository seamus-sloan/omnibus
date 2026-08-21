//! Book library reads (full + paginated), single-book fetch, FTS5 search,
//! cross-format merge, and Hardcover "readers also enjoyed" suggestions.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    BookDeletionManifest, DeleteBookFilesResult, EbookLibrary, EbookMetadata, LibraryPage,
    MergeBooksResult, SortDir, SortKey, SuggestionsResponse, ViewFilters,
};

#[cfg(feature = "server")]
use omnibus_db as db;

// `BookSuggestion` / `RawSuggestion` are only used in the server-side
// `rpc_get_suggestions` body; gate them so the web/mobile client builds don't
// flag an unused import.
#[cfg(feature = "server")]
use omnibus_shared::{BookDeletionImpact, BookSuggestion, RawSuggestion};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, AuthUser, PoolExt, WorkerExt};

/// Return the full indexed library (ebooks and audiobooks combined) for the
/// landing grid. Result is capped at `db::MAX_BOOKS_RETURNED` rows; clients
/// that need pagination should use `rpc_get_ebooks_page` instead.
#[get("/api/rpc/ebooks", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_ebooks() -> Result<EbookLibrary> {
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    // Served straight from the DB — the indexer is responsible for keeping
    // it up to date (startup + settings save triggers).
    //
    // Unified landing: ebooks + audiobooks live in the same `books` table
    // under different `library_id`s, so the unified grid is one query
    // over the union. The format facet in `ViewFilters` does the per-format
    // splitting on the client.
    //
    // Issue #81: the underlying `list_books` query is capped at
    // `db::MAX_BOOKS_RETURNED` rows so a multi-thousand-book install
    // can't bandwidth-DoS itself on every poll. Dioxus server functions
    // don't expose response headers, so this path can't currently
    // surface a "truncated" hint to the web client — the F1.3 spec
    // constrains the client-side sort/filter UX to ~10k books anyway,
    // which is well under the cap. Cursor pagination is the next step.
    Ok(db::library_from_db_combined(
        &pool.0,
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    )
    .await
    .map_err(|e| internal_rpc_error("list ebooks", e))?)
}

/// Keyset-paginated landing read (the web path's Option-B replacement for
/// the full-library `rpc_get_ebooks`). POST — like `rpc_search` — so the sort /
/// filter / cursor arguments ride in the JSON body that Dioxus `#[get]` server
/// functions can't carry.
///
/// The server owns the sort order, so the client drives `sort_key`/`sort_dir`
/// and appends pages by feeding `next_cursor` back as `cursor`. The first page
/// (`cursor == None`) also returns the full-library `total` (header count) and
/// the sidebar `facets`; later pages omit both so an infinite scroll doesn't
/// re-pay the aggregate cost — the client caches them.
///
/// `exclude_formats` is the caller's hidden-formats preference (from
/// `UserSummary::hidden_formats`) — client-passed so only the landing surface
/// applies it, and the first page's `total`/`hidden_count` report the visible
/// library size and the receipt.
#[post("/api/rpc/ebooks/page", pool: PoolExt, _user: AuthUser)]
#[allow(clippy::too_many_arguments)] // the macro adds pool/user to the six query knobs
pub async fn rpc_get_ebooks_page(
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: ViewFilters,
    exclude_formats: Vec<String>,
    cursor: Option<String>,
    limit: i64,
) -> Result<LibraryPage> {
    // Read-side clamp mirroring the REST twin: the exclusion is caller-
    // supplied, so cap it before it becomes SQL binds.
    let exclude_formats = omnibus_shared::sanitize_exclude_formats(exclude_formats);
    Ok(ebooks_page(
        &pool.0,
        sort_key,
        sort_dir,
        &filters,
        &exclude_formats,
        cursor.as_deref(),
        limit,
    )
    .await?)
}

/// Server-side body of [`rpc_get_ebooks_page`], extracted so the
/// cursor-decode and first-page-aggregates branches can be unit-tested
/// without the server-fn transport.
#[cfg(feature = "server")]
async fn ebooks_page(
    pool: &sqlx::SqlitePool,
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: &ViewFilters,
    exclude_formats: &[String],
    cursor: Option<&str>,
    limit: i64,
) -> Result<LibraryPage, ServerFnError> {
    let settings = db::get_settings(pool)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let ebook = settings.ebook_library_path;
    let audiobook = settings.audiobook_library_path;
    // The `EbookLibrary.path` convention: report the ebook path when set,
    // else the audiobook path (keys per-library view prefs on the client).
    let path = ebook.clone().or_else(|| audiobook.clone());
    let paths = db::collect_paths(ebook.as_deref(), audiobook.as_deref());

    // A malformed cursor is a client error. The web client only echoes a
    // server-issued cursor, so this is defensive — surface it rather than
    // silently restarting at the top. `CursorError::Malformed` carries a
    // caller-safe message, not raw internals, so it keeps its own text.
    let decoded = match cursor {
        Some(c) => match db::PageCursor::decode(c) {
            Ok(p) => Some(p),
            Err(e) => return Err(ServerFnError::new(e.to_string())),
        },
        None => None,
    };

    let page = db::list_books_page(
        pool,
        &paths,
        sort_key,
        sort_dir,
        filters,
        exclude_formats,
        decoded.as_ref(),
        limit,
    )
    .await
    .map_err(|e| internal_rpc_error("list books page", e))?;

    let (total, facets, hidden_count) = if decoded.is_none() {
        first_page_aggregates(pool, &paths, exclude_formats).await?
    } else {
        (None, None, None)
    };

    Ok(LibraryPage {
        path,
        books: page.books,
        next_cursor: page.next.map(|c| c.encode()),
        total,
        facets,
        hidden_count,
    })
}

/// `(total, facets, hidden_count)` — the shape `ebooks_page` folds straight
/// into `LibraryPage`.
#[cfg(feature = "server")]
type FirstPageAggregates = (
    Option<i64>,
    Option<omnibus_shared::FacetCounts>,
    Option<i64>,
);

/// First-page-only aggregates: the library-wide total (and, when an
/// exclusion is active, the hidden-count receipt beside it) plus the sidebar
/// facets. Both diff counts use default filters and differ only in the
/// exclusion, so with no exclusion this stays the single
/// `count_books_for_paths` query.
#[cfg(feature = "server")]
async fn first_page_aggregates(
    pool: &sqlx::SqlitePool,
    paths: &[&str],
    exclude_formats: &[String],
) -> Result<FirstPageAggregates, ServerFnError> {
    let all = db::count_books_for_paths(pool, paths)
        .await
        .map_err(|e| internal_rpc_error("count books", e))?;
    let (total, hidden) = if exclude_formats.is_empty() {
        (all, None)
    } else {
        let visible = db::count_books_page(pool, paths, &ViewFilters::default(), exclude_formats)
            .await
            .map_err(|e| internal_rpc_error("count visible books", e))?;
        (visible, Some(all - visible))
    };
    let facets = db::library_facets(pool, paths)
        .await
        .map_err(|e| internal_rpc_error("library facets", e))?;
    Ok((Some(total), Some(facets), hidden))
}

/// POST (not GET) for the same reason as `rpc_search`: Dioxus `#[get]`
/// server functions can't carry an argument body, so anything that needs
/// `uuid` rides as a JSON-bodied POST. The argument is the stable
/// `books.uuid` rather than the renumbering `books.id` so bookmarked
/// `/books/:uuid` URLs survive reindexes.
#[post("/api/rpc/ebook", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_ebook(uuid: String) -> Result<Option<EbookMetadata>> {
    let book = db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?;
    Ok(book)
}

/// Admin: merge the book `source_uuid` into `target_uuid` — the target
/// absorbs the source's files, links, identifiers, and progress; the
/// source row disappears. Returns the `merge_log` id (the undo handle)
/// and the surviving uuid. Same-format files are allowed (e.g. merging
/// five M4B books into one). Domain failures (`SameBook`, …) surface as
/// their display strings so the dialog can render them directly.
#[post("/api/rpc/merge-books", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_merge_books(source_uuid: String, target_uuid: String) -> Result<MergeBooksResult> {
    match db::merge_books(&pool.0, &source_uuid, &target_uuid, Some(admin.0.id)).await {
        Ok(out) => Ok(MergeBooksResult {
            merge_log_id: out.merge_log_id,
            target_uuid: out.target_uuid,
        }),
        // Domain failures keep their display string — see the doc comment above.
        Err(e @ (db::MergeError::SameBook | db::MergeError::BookNotFound(_))) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(e) => Err(internal_rpc_error("merge books", e).into()),
    }
}

/// Admin: reverse a merge recorded in `merge_log`. Returns the restored
/// (source) book's uuid.
#[post("/api/rpc/merge-books/undo", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_undo_merge(merge_log_id: i64) -> Result<String> {
    match db::undo_merge(&pool.0, merge_log_id).await {
        Ok(uuid) => Ok(uuid),
        Err(e @ (db::MergeError::LogNotFound | db::MergeError::AlreadyUndone)) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(e) => Err(internal_rpc_error("undo merge", e).into()),
    }
}

/// Admin: the book's deletable items (files + physical copies) and the user
/// data a total delete would take with them — everything the delete dialog
/// lists and confirms against.
#[post("/api/rpc/books/deletion-manifest", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_book_deletion_manifest(uuid: String) -> Result<BookDeletionManifest> {
    match db::book_deletion_manifest(&pool.0, &uuid).await {
        Ok(m) => Ok(BookDeletionManifest {
            files: m.files,
            copies: m.copies,
            impact: BookDeletionImpact {
                highlights: m.impact.highlights,
                journal_entries: m.impact.journal_entries,
                bookmarks: m.impact.bookmarks,
                reading_sessions: m.impact.reading_sessions,
                listening_sessions: m.impact.listening_sessions,
                ratings: m.impact.ratings,
                shelves: m.impact.shelves,
            },
        }),
        Err(e @ db::DeleteError::BookNotFound) => Err(ServerFnError::new(e.to_string()).into()),
        Err(e) => Err(internal_rpc_error("book deletion manifest", e).into()),
    }
}

/// Admin: delete the given items — `file_ids` are `book_files` rows (removed
/// from disk too), `copy_ids` are physical copies (un-recorded only). When
/// every item goes, the book and everything keyed to its uuid go with it —
/// irreversible, which is why the dialog confirms first.
#[post("/api/rpc/books/delete-files", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_delete_book_files(
    uuid: String,
    file_ids: Vec<i64>,
    copy_ids: Vec<i64>,
) -> Result<DeleteBookFilesResult> {
    match db::delete_book_items(&pool.0, &uuid, &file_ids, &copy_ids).await {
        Ok(out) => Ok(DeleteBookFilesResult {
            deleted_file_ids: out.deleted_file_ids,
            deleted_copy_ids: out.deleted_copy_ids,
            book_deleted: out.book_deleted,
        }),
        // Domain failures keep their display string so the dialog can render them.
        Err(
            e @ (db::DeleteError::BookNotFound
            | db::DeleteError::FileNotFound(_)
            | db::DeleteError::CopyNotFound(_)),
        ) => Err(ServerFnError::new(e.to_string()).into()),
        Err(e) => Err(internal_rpc_error("delete book files", e).into()),
    }
}

/// Admin: candidate search for the merge dialog. Same FTS5 query as
/// `rpc_search`, explicitly deduped by uuid for the shared-directory case and
/// capped small because the dialog shows only a handful of rows.
#[post("/api/rpc/merge-books/candidates", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_merge_candidates(q: String) -> Result<Vec<EbookMetadata>> {
    Ok(merge_candidates(&pool.0, &q).await?)
}

/// Server-side body of [`rpc_merge_candidates`], extracted so the
/// cross-library dedup and the 20-row cap can be unit-tested without the
/// server-fn transport.
#[cfg(feature = "server")]
async fn merge_candidates(
    pool: &sqlx::SqlitePool,
    q: &str,
) -> Result<Vec<EbookMetadata>, ServerFnError> {
    if omnibus_shared::search_query_too_long(q) {
        return Err(ServerFnError::new("query too long"));
    }
    let settings = db::get_settings(pool)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let mut out: Vec<EbookMetadata> = Vec::new();
    for path in [settings.ebook_library_path, settings.audiobook_library_path]
        .into_iter()
        .flatten()
    {
        out.extend(
            db::search_books(pool, &path, q)
                .await
                .map_err(|e| internal_rpc_error("search books", e))?,
        );
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|b| seen.insert(b.unique_identifier.clone()));
    out.truncate(20);
    Ok(out)
}

/// FTS5-backed search across the configured ebook and audiobook libraries.
/// Empty or whitespace-only `q` returns an empty library.
///
/// POST (not GET) so the query string can ride in the JSON body — Dioxus
/// `#[get]` server functions reject arg bodies because HTTP spec forbids
/// bodies on GET.
///
/// `search_books` is capped server-side at `db::MAX_BOOKS_RETURNED` hits.
/// Server functions can't expose response headers, so — unlike
/// `rpc_get_ebooks` — the search path threads the full hit count through
/// `EbookLibrary::total`, letting the web client show "N of M results"
/// when the vec is truncated.
#[post("/api/rpc/search", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_search(q: String) -> Result<EbookLibrary> {
    Ok(search_ebooks(&pool.0, &q).await?)
}

/// Server-side body of [`rpc_search`], extracted so the length-cap guard and
/// the FTS5 call can be unit-tested without the server-fn transport.
#[cfg(feature = "server")]
async fn search_ebooks(pool: &sqlx::SqlitePool, q: &str) -> Result<EbookLibrary, ServerFnError> {
    if omnibus_shared::search_query_too_long(q) {
        return Err(ServerFnError::new("query too long"));
    }
    let settings = db::get_settings(pool)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let ebook = settings.ebook_library_path;
    let audiobook = settings.audiobook_library_path;
    let paths = db::collect_paths(ebook.as_deref(), audiobook.as_deref());
    if paths.is_empty() {
        return Ok(EbookLibrary::default());
    }
    let path = paths[0].to_string();
    // Issue #241: single FTS5 pass returns the capped vec and the full count.
    let (books, total) = db::search_books_for_paths_with_total(pool, &paths, q)
        .await
        .map_err(|e| internal_rpc_error("search books", e))?;
    Ok(EbookLibrary {
        path: Some(path),
        books,
        error: None,
        total: Some(total),
    })
}

/// "Readers also enjoyed" for one book. Drives the cache de-duplication
/// state machine: returns `NotConfigured` when no Hardcover key is set, serves
/// the fresh/sticky cache when present, and enqueues a single background
/// resolution (marking `pending` *before* posting) so a refresh or a burst of
/// concurrent viewers never re-hits Hardcover. POST because it carries `uuid`.
#[post("/api/rpc/ebook-suggestions", pool: PoolExt, worker: WorkerExt, _user: AuthUser)]
pub async fn rpc_get_suggestions(uuid: String) -> Result<SuggestionsResponse> {
    let hardcover_key = db::effective_hardcover_api_key(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get hardcover key", e))?;
    if hardcover_key.is_none() {
        return Ok(SuggestionsResponse::NotConfigured);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let state = db::suggestion_state(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("suggestion state", e))?;
    let decision = db::decide(state, now);
    if decision.enqueue {
        // Mark pending before posting so concurrent callers can't each enqueue.
        db::mark_pending(&pool.0, &uuid)
            .await
            .map_err(|e| internal_rpc_error("mark suggestions pending", e))?;
        worker.0.post(db::worker::Task::ResolveSuggestions {
            book_uuid: uuid.clone(),
        });
    }
    if decision.serve {
        let items = db::get_suggestions(&pool.0, &uuid)
            .await
            .map_err(|e| internal_rpc_error("get suggestions", e))?
            .into_iter()
            .map(|c| {
                BookSuggestion::new(
                    &uuid,
                    c.rank,
                    RawSuggestion {
                        hardcover_id: c.hardcover_id,
                        hardcover_slug: c.hardcover_slug,
                        title: c.title,
                        author: c.author,
                        list_count: c.list_count,
                    },
                    c.has_cover,
                )
            })
            .collect();
        Ok(SuggestionsResponse::Ready { items })
    } else {
        Ok(SuggestionsResponse::Pending)
    }
}

// `server`-gated: exercises the extracted server-side bodies against an
// in-memory DB. CI runs this via `cargo test -p omnibus-frontend --features
// server`.
#[cfg(all(test, feature = "server"))]
mod tests;
