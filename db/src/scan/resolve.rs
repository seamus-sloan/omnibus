//! The matching ladder and check-in write composition.

use sqlx::{Row, SqlitePool};

use omnibus_shared::isbn::{normalize_isbn, IsbnError};
use omnibus_shared::metadata_lookup::ExternalBookMeta;
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::scan::{ScanBook, ScanOutcome};

use crate::author_photos::fetch_remote_image;
use crate::metadata_lookup::{lookup_isbn, MetadataLookupConfig, MetadataLookupError};
use crate::normalize::{normalize_author, normalize_title};
use crate::physical::{
    add_physical_copy, add_wishlist_entry, create_fileless_book, FilelessBook, FilelessCover,
    PhysicalError,
};

/// Errors from the scan flow.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// The input ISBN failed validation (length, chars, or check digit).
    #[error(transparent)]
    Isbn(#[from] IsbnError),
    /// A metadata provider was unreachable or unparseable.
    #[error(transparent)]
    Lookup(#[from] MetadataLookupError),
    /// A physical/wishlist write failed.
    #[error(transparent)]
    Physical(#[from] PhysicalError),
    /// `wishlist_add` was called with neither a `book_uuid` nor `meta`.
    #[error("wishlist add requires a book_uuid or meta")]
    MissingWishlistTarget,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Resolve a scanned/typed ISBN down the matching ladder.
///
/// 1. Normalize the ISBN (→ [`ScanError::Isbn`] on a bad input).
/// 2. Exact `book_identifiers` ISBN hit → `AlreadyOwned` / `InLibraryUnowned`.
/// 3. Otherwise online lookup; a normalized (title, author) hit is a
///    `CloseMatch` (never auto-resolved), else `NotInLibrary`; a provider miss
///    is `Unresolved`.
pub async fn resolve_scan(
    pool: &SqlitePool,
    raw_isbn: &str,
    config: &MetadataLookupConfig,
) -> Result<ScanOutcome, ScanError> {
    let isbn13 = normalize_isbn(raw_isbn)?;

    if let Some(book) = find_book_by_isbn(pool, &isbn13).await? {
        return Ok(if book.has_physical {
            ScanOutcome::AlreadyOwned { book }
        } else {
            ScanOutcome::InLibraryUnowned { book }
        });
    }

    match lookup_isbn(config, &isbn13).await? {
        Some(meta) => match find_book_by_norm(pool, &meta).await? {
            Some(book) => Ok(ScanOutcome::CloseMatch {
                book,
                scanned: meta,
            }),
            None => Ok(ScanOutcome::NotInLibrary { online: meta }),
        },
        None => Ok(ScanOutcome::Unresolved),
    }
}

/// Add a physical-only book from resolved external metadata: mint a fileless
/// book (cover fetched now, not at lookup time) and check in its first copy.
/// Returns the new book's uuid.
pub async fn add_physical_only(
    pool: &SqlitePool,
    meta: &ExternalBookMeta,
    note: Option<&str>,
    added_by_user_id: Option<i64>,
) -> Result<String, ScanError> {
    let uuid = create_fileless_from_meta(pool, meta).await?;
    // Store the canonical ISBN per copy (meta arrives over the wire — untrusted).
    let isbn = canonical_isbn(meta);
    add_physical_copy(pool, &uuid, isbn.as_deref(), added_by_user_id, note).await?;
    Ok(uuid)
}

/// Add a book to a user's physical wishlist — an existing library book
/// (`book_uuid`) or a new fileless book from `meta`. `book_uuid` takes
/// precedence when both are supplied. Returns the book's uuid.
pub async fn wishlist_add(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: Option<&str>,
    meta: Option<&ExternalBookMeta>,
    source: WishlistSource,
) -> Result<String, ScanError> {
    let uuid = match (book_uuid, meta) {
        (Some(uuid), _) => uuid.to_string(),
        (None, Some(meta)) => create_fileless_from_meta(pool, meta).await?,
        (None, None) => return Err(ScanError::MissingWishlistTarget),
    };
    let entry = add_wishlist_entry(pool, user_id, &uuid, source).await?;
    Ok(entry.book_uuid)
}

/// Mint a fileless book from external metadata, fetching its cover now.
///
/// `meta` crosses the HTTP boundary (`AddPhysicalOnly`/`WishlistAdd` bodies), so
/// its `cover_url` is client-controlled: the cover fetch goes through the
/// SSRF-guarded [`fetch_remote_image`] (strict — public IPs only, size-capped)
/// rather than a raw request, and the ISBN is re-canonicalized before storage.
async fn create_fileless_from_meta(
    pool: &SqlitePool,
    meta: &ExternalBookMeta,
) -> Result<String, ScanError> {
    let cover = match &meta.cover_url {
        Some(url) => fetch_remote_image(url)
            .await
            .ok()
            .map(|(mime, bytes)| FilelessCover { mime, bytes }),
        None => None,
    };
    let uuid = create_fileless_book(
        pool,
        FilelessBook {
            title: meta.title.clone(),
            authors: meta.authors.clone(),
            isbn: canonical_isbn(meta),
            pubdate: meta.year.clone(),
            description: meta.description.clone(),
            cover,
        },
    )
    .await?;
    Ok(uuid)
}

/// Canonicalize `meta.isbn13` (untrusted wire input) to a 13-digit ISBN, or
/// `None` if it doesn't validate — better to store no identifier than a bad one.
fn canonical_isbn(meta: &ExternalBookMeta) -> Option<String> {
    normalize_isbn(&meta.isbn13).ok()
}

/// Exact-identifier rung: the book carrying this ISBN in `book_identifiers`.
///
/// OPF identifiers are stored losslessly, so the scheme is free-form
/// (`ISBN`, `isbn`, `urn:isbn`) and the value may carry hyphens/spaces. Match
/// any `%isbn%` scheme (case-insensitive) and strip separators from the stored
/// value before comparing — mirroring `derive_isbn13`'s tolerance — so a real
/// library ISBN isn't missed and mis-routed to online lookup.
async fn find_book_by_isbn(
    pool: &SqlitePool,
    isbn13: &str,
) -> Result<Option<ScanBook>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT b.uuid, b.title, b.has_cover,
                (SELECT group_concat(a.name, ', ')
                   FROM books_authors_link bal JOIN authors a ON a.id = bal.author
                  WHERE bal.book = b.id ORDER BY bal.position) AS authors,
                EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid) AS has_physical
           FROM books b
           JOIN book_identifiers bi ON bi.book_id = b.id
          WHERE bi.scheme LIKE '%isbn%'
            AND REPLACE(REPLACE(bi.value, '-', ''), ' ', '') = ?1
          LIMIT 1",
    )
    .bind(isbn13)
    .fetch_optional(pool)
    .await?;
    // No caller on this rung reads `isbn`, so skip the correlated subquery.
    Ok(row.map(|r| row_to_scan_book(r, None)))
}

/// Fuzzy rung: a single library book whose normalized (title, author) matches
/// the resolved metadata. `None` unless exactly one candidate matches — an
/// ambiguous or absent match is not a close match.
async fn find_book_by_norm(
    pool: &SqlitePool,
    meta: &ExternalBookMeta,
) -> Result<Option<ScanBook>, sqlx::Error> {
    let (Some(title_norm), Some(author_norm)) = (
        normalize_title(&meta.title),
        meta.authors.first().and_then(|a| normalize_author(a)),
    ) else {
        return Ok(None);
    };
    let rows = sqlx::query(
        "SELECT b.uuid, b.title, b.has_cover,
                (SELECT group_concat(a.name, ', ')
                   FROM books_authors_link bal JOIN authors a ON a.id = bal.author
                  WHERE bal.book = b.id ORDER BY bal.position) AS authors,
                EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid) AS has_physical,
                (SELECT REPLACE(REPLACE(bi2.value, '-', ''), ' ', '')
                   FROM book_identifiers bi2
                  WHERE bi2.book_id = b.id AND bi2.scheme LIKE '%isbn%'
                    AND REPLACE(REPLACE(bi2.value, '-', ''), ' ', '')
                        GLOB '[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
                  ORDER BY bi2.rowid LIMIT 1) AS isbn
           FROM books b
          WHERE b.title_norm = ?1 AND b.author_norm = ?2
          ORDER BY b.id LIMIT 2",
    )
    .bind(title_norm)
    .bind(author_norm)
    .fetch_all(pool)
    .await?;
    // Exactly one candidate is a close match; zero or many is not.
    Ok(if rows.len() == 1 {
        rows.into_iter().next().map(|r| {
            let isbn = r.get::<Option<String>, _>("isbn").filter(|s| !s.is_empty());
            row_to_scan_book(r, isbn)
        })
    } else {
        None
    })
}

fn row_to_scan_book(r: sqlx::sqlite::SqliteRow, isbn: Option<String>) -> ScanBook {
    let uuid: String = r.get("uuid");
    let has_cover: i64 = r.get("has_cover");
    let authors: Option<String> = r.get("authors");
    let has_physical: i64 = r.get("has_physical");
    ScanBook {
        cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
        isbn,
        authors: authors
            .filter(|s| !s.is_empty())
            .map(|s| s.split(", ").map(str::to_string).collect())
            .unwrap_or_default(),
        title: r.get("title"),
        has_physical: has_physical != 0,
        uuid,
    }
}
