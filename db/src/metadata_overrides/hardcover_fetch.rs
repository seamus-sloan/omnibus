//! "Fetch from Hardcover" — read a book's canonical title / author /
//! description / series / ISBN-13 for the metadata-edit page's preview panel
//! without writing anything. Applying the previewed fields is the caller's job,
//! via [`super::upsert_metadata_overrides`] / [`super::merge_metadata_overrides`].
//! Cover art is out of scope: nothing here can apply an overridden cover image.

#[cfg(test)]
mod tests;

use sqlx::SqlitePool;

use omnibus_shared::metadata_fetch::{FetchedBookMetadata, HardcoverFetchResult};

use crate::suggestions::cascade::extract_isbns;
use crate::suggestions::hardcover::{fetch_book_details, resolve_book, HardcoverConfig};

/// Look up `book_uuid` on Hardcover and return its canonical metadata for
/// preview. `Ok(HardcoverFetchResult::NotConfigured)` when no Hardcover key
/// is set (mirrors [`crate::suggestions::cascade::resolve`]) — the caller is
/// expected to hide the "Fetch from Hardcover" action in that case, but
/// staying a clean no-op keeps this safe if it doesn't.
pub async fn fetch_hardcover_metadata(
    pool: &SqlitePool,
    book_uuid: &str,
) -> anyhow::Result<HardcoverFetchResult> {
    let Some(key) = crate::settings::effective_hardcover_api_key(pool).await? else {
        return Ok(HardcoverFetchResult::NotConfigured);
    };
    fetch_hardcover_metadata_with(pool, book_uuid, &HardcoverConfig::new(key)).await
}

/// [`fetch_hardcover_metadata`] with an injectable Hardcover config (tests
/// point it at a local `wiremock` server). Resolves the book to a Hardcover
/// edition (ISBN-first, title fallback — the same ladder [`resolve_book`]
/// gives the suggestions ranking path), then hydrates the full field set.
pub async fn fetch_hardcover_metadata_with(
    pool: &SqlitePool,
    book_uuid: &str,
    config: &HardcoverConfig,
) -> anyhow::Result<HardcoverFetchResult> {
    let Some(book) = crate::get_book_by_uuid(pool, book_uuid).await? else {
        anyhow::bail!("book not found: {book_uuid}");
    };
    let title = book.title.clone().unwrap_or_default();
    let author = book.creators.first().map(|c| c.name.clone());
    let isbns = extract_isbns(&book);

    let Some(resolved) = resolve_book(config, &isbns, &title, author.as_deref()).await? else {
        return Ok(HardcoverFetchResult::NotFound);
    };
    let Some(details) = fetch_book_details(config, resolved.id).await? else {
        // Resolved an id, but it no longer answers a detail lookup (deleted
        // or merged on Hardcover's side between the two calls) — a miss for
        // this purpose, same as never having resolved at all.
        return Ok(HardcoverFetchResult::NotFound);
    };

    Ok(HardcoverFetchResult::Found(FetchedBookMetadata {
        title: details.title,
        authors: details.authors,
        description: details.description,
        series: details.series,
        series_index: details.series_index,
        isbn13: details.isbn13,
    }))
}
