//! "Fetch book summary" — pull a description from an external provider on
//! demand (an explicit user action, not the background worker). Hardcover is
//! preferred when a key is configured; OpenLibrary is the keyless fallback.
//! The caller drives the cascade one source at a time (via [`fetch_summary`])
//! so the UI can show a per-source status message.

pub mod openlibrary;

#[cfg(test)]
mod tests;

use sqlx::SqlitePool;

use omnibus_shared::summary::SummarySource;

use crate::suggestions::cascade::extract_isbns;
use crate::suggestions::hardcover::{book_description, resolve_book, HardcoverConfig};
use openlibrary::OpenLibrarySummaryConfig;

/// Fetch a summary for `book_uuid` from `source`. `Ok(None)` is a clean miss
/// (the source has no summary for this book) the caller can cascade past;
/// `Ok(Some(text))` is a hit. For [`SummarySource::Hardcover`] with no
/// configured key this returns `Ok(None)` — the caller is expected to skip
/// Hardcover in that case, but staying a no-op keeps it safe if it doesn't.
pub async fn fetch_summary(
    pool: &SqlitePool,
    book_uuid: &str,
    source: SummarySource,
) -> anyhow::Result<Option<String>> {
    match source {
        SummarySource::Hardcover => {
            let Some(key) = crate::settings::effective_hardcover_api_key(pool).await? else {
                return Ok(None);
            };
            fetch_summary_with(
                pool,
                book_uuid,
                source,
                &HardcoverConfig::new(key),
                &OpenLibrarySummaryConfig::default(),
            )
            .await
        }
        SummarySource::OpenLibrary => {
            fetch_summary_with(
                pool,
                book_uuid,
                source,
                // No Hardcover call is made on this branch; the config is unused.
                &HardcoverConfig::new(String::new()),
                &OpenLibrarySummaryConfig::default(),
            )
            .await
        }
    }
}

/// [`fetch_summary`] with injectable provider configs (tests point these at a
/// local `wiremock` server). Resolves the book's title/author/ISBNs once, then
/// dispatches to the requested provider.
pub async fn fetch_summary_with(
    pool: &SqlitePool,
    book_uuid: &str,
    source: SummarySource,
    hardcover: &HardcoverConfig,
    openlibrary: &OpenLibrarySummaryConfig,
) -> anyhow::Result<Option<String>> {
    let Some(book) = crate::get_book_by_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let title = book.title.clone().unwrap_or_default();
    let author = book.creators.first().map(|c| c.name.clone());
    let isbns = extract_isbns(&book);

    match source {
        SummarySource::Hardcover => {
            fetch_hardcover(hardcover, &isbns, &title, author.as_deref()).await
        }
        SummarySource::OpenLibrary => {
            openlibrary::fetch(openlibrary, &isbns, &title, author.as_deref()).await
        }
    }
}

/// Resolve the library book to a Hardcover book (ISBN-first, title fallback),
/// then read its long-form description.
async fn fetch_hardcover(
    config: &HardcoverConfig,
    isbns: &[String],
    title: &str,
    author: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(resolved) = resolve_book(config, isbns, title, author).await? else {
        return Ok(None);
    };
    Ok(book_description(config, resolved.id).await?)
}
