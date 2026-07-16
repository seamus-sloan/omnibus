//! Book library reads (full + paginated), single-book fetch, FTS5 search,
//! cross-format merge, and Hardcover "readers also enjoyed" suggestions.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    EbookLibrary, EbookMetadata, LibraryPage, MergeBooksResult, SortDir, SortKey,
    SuggestionsResponse, ViewFilters,
};

#[cfg(feature = "server")]
use omnibus_db as db;

// `BookSuggestion` / `RawSuggestion` are only used in the server-side
// `rpc_get_suggestions` body; gate them so the web/mobile client builds don't
// flag an unused import.
#[cfg(feature = "server")]
use omnibus_shared::{BookSuggestion, RawSuggestion};

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
#[post("/api/rpc/ebooks/page", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_ebooks_page(
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: ViewFilters,
    cursor: Option<String>,
    limit: i64,
) -> Result<LibraryPage> {
    let settings = db::get_settings(&pool.0)
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
    let decoded = match cursor.as_deref() {
        Some(c) => match db::PageCursor::decode(c) {
            Ok(p) => Some(p),
            Err(e) => return Err(ServerFnError::new(e.to_string()).into()),
        },
        None => None,
    };

    let page = db::list_books_page(
        &pool.0,
        &paths,
        sort_key,
        sort_dir,
        &filters,
        decoded.as_ref(),
        limit,
    )
    .await
    .map_err(|e| internal_rpc_error("list books page", e))?;

    let (total, facets) = if decoded.is_none() {
        (
            Some(
                db::count_books_for_paths(&pool.0, &paths)
                    .await
                    .map_err(|e| internal_rpc_error("count books", e))?,
            ),
            Some(
                db::library_facets(&pool.0, &paths)
                    .await
                    .map_err(|e| internal_rpc_error("library facets", e))?,
            ),
        )
    } else {
        (None, None)
    };

    Ok(LibraryPage {
        path,
        books: page.books,
        next_cursor: page.next.map(|c| c.encode()),
        total,
        facets,
    })
}

/// POST (not GET) for the same reason as `rpc_search`: Dioxus `#[get]`
/// server functions can't carry an argument body, so anything that needs
/// `uuid` rides as a JSON-bodied POST. The argument is the stable
/// `books.uuid` rather than the renumbering `books.id` so bookmarked
/// `/books/:uuid` URLs survive reindexes.
#[post("/api/rpc/ebook", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_ebook(uuid: String) -> Result<Option<EbookMetadata>> {
    Ok(db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?)
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

/// Admin: candidate search for the merge dialog. Same FTS5 query as
/// `rpc_search`, explicitly deduped by uuid for the shared-directory case and
/// capped small because the dialog shows only a handful of rows.
#[post("/api/rpc/merge-books/candidates", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_merge_candidates(q: String) -> Result<Vec<EbookMetadata>> {
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let mut out: Vec<EbookMetadata> = Vec::new();
    for path in [settings.ebook_library_path, settings.audiobook_library_path]
        .into_iter()
        .flatten()
    {
        out.extend(
            db::search_books(&pool.0, &path, &q)
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
    let settings = db::get_settings(&pool.0)
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
    let (books, total) = db::search_books_for_paths_with_total(&pool.0, &paths, &q)
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
