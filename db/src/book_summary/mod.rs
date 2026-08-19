//! "Fetch book summary" — pull a description from an external provider on an
//! explicit user action, not the background worker. [`summary_source_plan`]
//! computes the ordered cascade (Hardcover when keyed, Google Books, then Open
//! Library only when unkeyed); the caller drives it one source at a time via
//! [`fetch_summary`] so the UI can show a per-source status message.

pub mod googlebooks;
pub mod openlibrary;

#[cfg(test)]
mod tests;

use sqlx::SqlitePool;

use omnibus_shared::summary::SummarySource;

use crate::suggestions::cascade::extract_isbns;
use crate::suggestions::hardcover::{book_description, resolve_book, HardcoverConfig};
use googlebooks::GoogleBooksSummaryConfig;
use openlibrary::OpenLibrarySummaryConfig;

/// The ordered summary-source cascade for the current settings. Hardcover leads
/// when a key is configured; Google Books is always tried (it answers keyless,
/// on a shared quota); Open Library is the fallback *only* when no keys exist —
/// once any provider key is present we trust the keyed providers over it.
///
/// The only failure is a settings read, so the typed `SettingsError`
/// propagates directly rather than being erased into `anyhow`.
pub async fn summary_source_plan(
    pool: &SqlitePool,
) -> Result<Vec<SummarySource>, crate::settings::SettingsError> {
    let hardcover = crate::settings::hardcover_key_status(pool)
        .await?
        .configured;
    let google_books = crate::settings::google_books_key_status(pool)
        .await?
        .configured;

    let mut sources = Vec::with_capacity(3);
    if hardcover {
        sources.push(SummarySource::Hardcover);
    }
    sources.push(SummarySource::GoogleBooks);
    if !hardcover && !google_books {
        sources.push(SummarySource::OpenLibrary);
    }
    Ok(sources)
}

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
                &GoogleBooksSummaryConfig::default(),
                &OpenLibrarySummaryConfig::default(),
            )
            .await
        }
        SummarySource::GoogleBooks => {
            // Google Books answers with or without a key; pass the effective key
            // (settings wins over env) so keyed instances dodge the anon quota.
            let key = crate::settings::effective_google_books_api_key(pool).await?;
            fetch_summary_with(
                pool,
                book_uuid,
                source,
                &HardcoverConfig::new(String::new()),
                &GoogleBooksSummaryConfig::live(key),
                &OpenLibrarySummaryConfig::default(),
            )
            .await
        }
        SummarySource::OpenLibrary => {
            fetch_summary_with(
                pool,
                book_uuid,
                source,
                // No Hardcover/Google Books call is made on this branch; those
                // configs are unused.
                &HardcoverConfig::new(String::new()),
                &GoogleBooksSummaryConfig::default(),
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
    googlebooks: &GoogleBooksSummaryConfig,
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
        SummarySource::GoogleBooks => {
            googlebooks::fetch(googlebooks, &isbns, &title, author.as_deref()).await
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
