//! Resolution orchestration for suggestions, mirroring
//! `author_photos::cascade`: a single [`resolve`] entry point the worker drives
//! (via [`crate::worker::Task::ResolveSuggestions`]) plus a [`resolve_with`]
//! that takes injectable Hardcover + image configs for tests.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;

use crate::author_photos::{fetch_remote_image_with, RemoteImageConfig};
use crate::suggestions::data::{
    replace_suggestions, suggestion_state, NewSuggestion, SuggestionState, SUGGESTIONS_TTL_SECS,
};
use crate::suggestions::filter::filter_candidates;
use crate::suggestions::hardcover::{
    co_listed_counts, curated_list_ids, fetch_candidates, resolve_book, HardcoverConfig,
};

/// Final strip size — at most 10 survivors are cached.
const FINAL_LIMIT: usize = 10;

/// Current unix time in seconds.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Resolve and cache suggestions for `book_uuid`. Reads the effective Hardcover
/// key (settings → env); a missing key is a no-op (the UI shows a connect
/// message). Driven by [`crate::worker::Task::ResolveSuggestions`].
pub async fn resolve(pool: &SqlitePool, book_uuid: &str) -> anyhow::Result<()> {
    let Some(key) = crate::settings::effective_hardcover_api_key(pool).await? else {
        return Ok(());
    };
    resolve_with(
        pool,
        book_uuid,
        &HardcoverConfig::new(key),
        &RemoteImageConfig::default(),
    )
    .await
}

/// [`resolve`] with injectable Hardcover + remote-image configs (tests point
/// these at a local `wiremock` server).
///
/// Flow: resolve the library book to a Hardcover book (ISBN-first, title
/// fallback) → collect its curated lists → rank co-listed books → filter to
/// different-author / different-series / entry-point survivors → fetch cover
/// bytes → cache the top 10. A clean miss writes the sticky `empty` marker; a
/// transient network error leaves the marker untouched so the debounce window
/// re-posts later.
pub async fn resolve_with(
    pool: &SqlitePool,
    book_uuid: &str,
    config: &HardcoverConfig,
    image_config: &RemoteImageConfig,
) -> anyhow::Result<()> {
    // A sibling task (serialized behind us on the same resource key) may have
    // already produced a fresh result — skip the network work if so. We do NOT
    // skip on `pending`: that's our own in-flight marker, set by the RPC.
    if let Some((state, at)) = suggestion_state(pool, book_uuid).await? {
        let fresh = now_secs().saturating_sub(at) <= SUGGESTIONS_TTL_SECS;
        if matches!(state, SuggestionState::Resolved | SuggestionState::Empty) && fresh {
            return Ok(());
        }
    }

    let Some(book) = crate::get_book_by_uuid(pool, book_uuid).await? else {
        // Book vanished (reindex/ghost) — nothing to resolve.
        return Ok(());
    };
    let title = book.title.clone().unwrap_or_default();
    let author = book.creators.first().map(|c| c.name.clone());
    let isbns = extract_isbns(&book);

    // Resolve the library book to a Hardcover book.
    let Some(resolved) = resolve_book(config, &isbns, &title, author.as_deref()).await? else {
        // Hardcover has no matching book — sticky negative until the TTL lapses.
        replace_suggestions(pool, book_uuid, &[], SuggestionState::Empty).await?;
        return Ok(());
    };

    // Collect curated lists → rank co-listed books → hydrate the deep pool.
    let list_ids = curated_list_ids(config, resolved.id).await?;
    let ranked = co_listed_counts(config, &list_ids, resolved.id).await?;
    let candidates = fetch_candidates(config, &ranked).await?;

    // Filter to different-author / different-series / entry-point survivors.
    let survivors = filter_candidates(
        &candidates,
        &resolved.author_names,
        resolved.series.as_deref(),
        FINAL_LIMIT,
    );

    // Build cache rows, fetching cover bytes best-effort (a cover miss must not
    // drop the suggestion — `Cover` falls back to a typographic plate).
    let mut rows: Vec<NewSuggestion> = Vec::with_capacity(survivors.len());
    for c in survivors {
        let cover = match &c.cover_url {
            Some(url) => fetch_remote_image_with(url, image_config).await.ok(),
            None => None,
        };
        // Persist a cover only when the fetch returned real bytes. An empty
        // body (e.g. a 200 with no payload) would set `cover_bytes != NULL` —
        // making `get_suggestions` report `has_cover = true` while the cover
        // endpoint serves a 404 (it rejects empty bytes). Treat empty as none.
        let (cover_mime, cover_bytes) = match cover {
            Some((mime, bytes)) if !bytes.is_empty() => (Some(mime), Some(bytes)),
            _ => (None, None),
        };
        rows.push(NewSuggestion {
            hardcover_id: c.hardcover_id,
            hardcover_slug: c.slug,
            title: c.title,
            author: c.author,
            list_count: c.list_count,
            cover_mime,
            cover_bytes,
        });
    }

    let state = if rows.is_empty() {
        SuggestionState::Empty
    } else {
        SuggestionState::Resolved
    };
    replace_suggestions(pool, book_uuid, &rows, state).await?;
    Ok(())
}

/// Pull candidate ISBNs from a book's identifiers. Accepts identifiers whose
/// scheme names ISBN, plus any value that *looks* like a 10/13-digit ISBN
/// (covers the common `scheme = "unknown"` case). Hyphens/spaces stripped.
fn extract_isbns(book: &omnibus_shared::EbookMetadata) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for ident in &book.identifiers {
        let scheme_is_isbn = ident
            .scheme
            .as_deref()
            .map(|s| s.to_lowercase().contains("isbn"))
            .unwrap_or(false);
        let cleaned: String = ident
            .value
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
            .collect();
        let looks_like_isbn = matches!(cleaned.len(), 10 | 13);
        if (scheme_is_isbn || looks_like_isbn) && !cleaned.is_empty() && !out.contains(&cleaned) {
            out.push(cleaned);
        }
    }
    out
}
