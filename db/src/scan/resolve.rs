//! The matching ladder and check-in write composition.

use omnibus_shared::isbn::{normalize_isbn, IsbnError};
use omnibus_shared::metadata_lookup::ExternalBookMeta;
use omnibus_shared::physical::{PhysicalCopy, WishlistSource};
use omnibus_shared::scan::{ScanBook, ScanOutcome};
use sqlx::{Row, SqlitePool};

use crate::author_photos::{fetch_remote_image_with, RemoteImageConfig};
use crate::metadata_lookup::{
    openlibrary_enrich, provider_cover_image_config, search_provider_by_isbn, MetadataLookupConfig,
    MetadataLookupError,
};
use crate::normalize::{normalize_author, normalize_title};
use crate::physical::{
    add_physical_copy, add_wishlist_entry, create_fileless_book, get_wishlist_entry, FilelessBook,
    FilelessCover, PhysicalError,
};

/// How many library rows one close match may offer the reader to choose from.
///
/// Two rows for a single work — an EPUB and the audiobook the indexer never
/// attached to it — is the case the picker exists for, and a handful covers
/// every realistic variant of it. A predicate that sprays past this is a
/// pathological match, not a choice anyone can make, so the list is capped
/// rather than shipped whole.
pub(super) const MAX_CLOSE_MATCH_CANDIDATES: usize = 5;

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
/// 2. Exact `book_identifiers` ISBN hit → `AlreadyOwned` / `OnWishlist` /
///    `InLibraryUnowned`.
/// 3. Otherwise online lookup; a normalized (title, author) hit is a
///    `CloseMatch` carrying every row that matched (never auto-resolved), else
///    `NotInLibrary`; a provider miss is `Unresolved`.
///
/// `user_id` scopes the wishlist check: a book the caller already wishlists (no
/// physical copy yet) resolves to `OnWishlist` so the flow opens its detail page
/// rather than the "own it digitally" confirm screen. Checking in a copy clears
/// the book from every user's wishlist, so a book with a physical copy is never
/// also wishlisted — the wishlist check only bites the no-physical branch.
pub async fn resolve_scan(
    pool: &SqlitePool,
    user_id: i64,
    raw_isbn: &str,
    config: &MetadataLookupConfig,
) -> Result<ScanOutcome, ScanError> {
    let isbn13 = normalize_isbn(raw_isbn)?;

    if let Some(book) = find_book_by_isbn(pool, &isbn13).await? {
        return library_outcome(pool, user_id, book).await;
    }

    match search_provider_by_isbn(config, &isbn13).await? {
        Some(meta) => match find_book_by_norm(pool, &meta).await? {
            Some((book, others)) => Ok(ScanOutcome::CloseMatch {
                book,
                others,
                scanned: meta,
            }),
            None => Ok(ScanOutcome::NotInLibrary { online: meta }),
        },
        None => Ok(ScanOutcome::Unresolved),
    }
}

/// Resolve a picked title-search candidate down the same ladder as
/// [`resolve_scan`], minus the provider ISBN lookup — the metadata is already
/// in hand, and a re-lookup could miss on a flaky provider and turn a book
/// the search just surfaced back into "unresolved".
///
/// Series / first-publish enrichment still runs (search results don't carry
/// those fields), so the outcome screens show the same detail either way a
/// book was found.
pub async fn resolve_meta(
    pool: &SqlitePool,
    user_id: i64,
    meta: &ExternalBookMeta,
    config: &MetadataLookupConfig,
) -> Result<ScanOutcome, ScanError> {
    // `meta` is untrusted wire input, so its ISBN is canonicalized once here
    // and the raw string never leaves this function: it reaches neither SQL
    // nor — since enrichment interpolates it into a provider URL path — an
    // outbound request. `None` means it didn't validate, which skips both.
    let isbn13 = canonical_isbn(meta);

    if let Some(isbn13) = &isbn13 {
        if let Some(book) = find_book_by_isbn(pool, isbn13).await? {
            return library_outcome(pool, user_id, book).await;
        }
    }

    let mut meta = meta.clone();
    if let Some(isbn13) = &isbn13 {
        let enrichment = openlibrary_enrich(config, isbn13).await;
        meta.series = meta.series.or(enrichment.series);
        meta.first_publish_year = meta.first_publish_year.or(enrichment.first_publish_year);
    }

    match find_book_by_norm(pool, &meta).await? {
        Some((book, others)) => Ok(ScanOutcome::CloseMatch {
            book,
            others,
            scanned: meta,
        }),
        None => Ok(ScanOutcome::NotInLibrary { online: meta }),
    }
}

/// Map an exact-identifier library hit onto its outcome: physically owned,
/// wishlisted by this caller, or in the library digitally only.
async fn library_outcome(
    pool: &SqlitePool,
    user_id: i64,
    book: ScanBook,
) -> Result<ScanOutcome, ScanError> {
    if book.has_physical {
        return Ok(ScanOutcome::AlreadyOwned { book });
    }
    let on_wishlist = get_wishlist_entry(pool, user_id, &book.uuid)
        .await?
        .is_some();
    Ok(if on_wishlist {
        ScanOutcome::OnWishlist { book }
    } else {
        ScanOutcome::InLibraryUnowned { book }
    })
}

/// Check in a physical copy of a library book, canonicalizing the scanned ISBN
/// before it is stored. Returns the inserted copy.
///
/// The ISBN reaches this call from a client — a barcode decode, a keypad, or a
/// provider record — so it is folded to the same 13-digit form
/// [`find_book_by_isbn`] compares against. Without that, a copy checked in
/// under an ISBN-10 would be invisible to the re-scan of the very barcode that
/// filed it. An ISBN that doesn't validate is dropped rather than stored, the
/// same way [`add_physical_only`] drops one: no identifier beats a wrong one.
pub async fn check_in_copy(
    pool: &SqlitePool,
    book_uuid: &str,
    isbn: Option<&str>,
    added_by_user_id: Option<i64>,
    note: Option<&str>,
) -> Result<PhysicalCopy, ScanError> {
    let isbn = isbn.and_then(|raw| normalize_isbn(raw).ok());
    Ok(add_physical_copy(pool, book_uuid, isbn.as_deref(), added_by_user_id, note).await?)
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
    add_physical_only_with(
        pool,
        meta,
        note,
        added_by_user_id,
        &provider_cover_image_config(false),
    )
    .await
}

/// [`add_physical_only`] with an injectable cover-fetch config.
///
/// `pub(crate)` deliberately: the only reason to pass anything other than
/// [`provider_cover_image_config`] is a test pointing the fetch at a loopback
/// `wiremock` origin, and that config relaxes the SSRF address gate.
pub(crate) async fn add_physical_only_with(
    pool: &SqlitePool,
    meta: &ExternalBookMeta,
    note: Option<&str>,
    added_by_user_id: Option<i64>,
    image_config: &RemoteImageConfig,
) -> Result<String, ScanError> {
    let uuid = create_fileless_from_meta(pool, meta, image_config).await?;
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
    wishlist_add_with(
        pool,
        user_id,
        book_uuid,
        meta,
        source,
        &provider_cover_image_config(false),
    )
    .await
}

/// [`wishlist_add`] with an injectable cover-fetch config — `pub(crate)` on the
/// same terms as [`add_physical_only_with`].
pub(crate) async fn wishlist_add_with(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: Option<&str>,
    meta: Option<&ExternalBookMeta>,
    source: WishlistSource,
    image_config: &RemoteImageConfig,
) -> Result<String, ScanError> {
    let uuid = match (book_uuid, meta) {
        (Some(uuid), _) => uuid.to_string(),
        (None, Some(meta)) => create_fileless_from_meta(pool, meta, image_config).await?,
        (None, None) => return Err(ScanError::MissingWishlistTarget),
    };
    let entry = add_wishlist_entry(pool, user_id, &uuid, source).await?;
    Ok(entry.book_uuid)
}

/// Mint a fileless book from external metadata, fetching its cover now.
///
/// `meta` crosses the HTTP boundary (`AddPhysicalOnly`/`WishlistAdd` bodies), so
/// its `cover_url` is client-controlled. The fetch therefore runs under
/// [`provider_cover_image_config`] — the same terms the metadata editor's
/// cover-apply uses: HTTPS only, hosts limited to the provider catalog's, the
/// SSRF address gate before any connect, a size cap, and a bounded redirect
/// follow. The follow is not optional: Open Library's cover CDN 302s twice
/// before serving bytes, so a zero-hop config drops every cover it publishes.
/// The ISBN is re-canonicalized before storage.
async fn create_fileless_from_meta(
    pool: &SqlitePool,
    meta: &ExternalBookMeta,
    image_config: &RemoteImageConfig,
) -> Result<String, ScanError> {
    let cover = match &meta.cover_url {
        Some(url) => match fetch_remote_image_with(url, image_config).await {
            Ok((mime, bytes)) => Some(FilelessCover { mime, bytes }),
            // Non-fatal — a missing cover must not fail the check-in — but
            // logged: a cover the reader watched render on the check-in card
            // and then lost is otherwise invisible to the operator.
            Err(e) => {
                tracing::warn!(url, error = %e, "cover fetch failed; minting book without it");
                None
            }
        },
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

/// Exact-identifier rung: the book carrying this ISBN in `book_identifiers`,
/// or failing that the book a physical copy carrying it was checked in against.
///
/// OPF identifiers are stored losslessly, so the scheme is free-form
/// (`ISBN`, `isbn`, `urn:isbn`) and the value may carry hyphens/spaces. Match
/// any `%isbn%` scheme (case-insensitive) and strip separators from the stored
/// value before comparing — mirroring `derive_isbn13`'s tolerance — so a real
/// library ISBN isn't missed and mis-routed to online lookup.
///
/// The `physical_copies` arm is what makes a hand-linked copy stick. A print
/// edition's barcode is a different ISBN from the ebook's, so a reader who
/// linked one to a library book by hand would otherwise be asked the same
/// question on every later scan of that same barcode. It ranks *after* the
/// identifier arm so a book that genuinely publishes the ISBN still wins.
async fn find_book_by_isbn(
    pool: &SqlitePool,
    isbn13: &str,
) -> Result<Option<ScanBook>, sqlx::Error> {
    let cols = "b.uuid AS uuid, b.title AS title, b.has_cover AS has_cover,
                (SELECT group_concat(a.name, ', ')
                   FROM books_authors_link bal JOIN authors a ON a.id = bal.author
                  WHERE bal.book = b.id ORDER BY bal.position) AS authors,
                EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid)
                    AS has_physical";
    let row = sqlx::query(&format!(
        "SELECT {cols}, 0 AS rung
           FROM books b
           JOIN book_identifiers bi ON bi.book_id = b.id
          WHERE bi.scheme LIKE '%isbn%'
            AND REPLACE(REPLACE(bi.value, '-', ''), ' ', '') = ?1
         UNION ALL
         SELECT {cols}, 1 AS rung
           FROM books b
           JOIN physical_copies c ON c.book_uuid = b.uuid
          WHERE REPLACE(REPLACE(c.isbn, '-', ''), ' ', '') = ?1
          ORDER BY rung LIMIT 1"
    ))
    .bind(isbn13)
    .fetch_optional(pool)
    .await?;
    // No caller on this rung reads `isbn`, so skip the correlated subquery.
    Ok(row.map(|r| row_to_scan_book(r, None)))
}

/// Fuzzy rung: the library books whose normalized (title, author) matches the
/// resolved metadata, as `(first, rest)` so a hit is non-empty by construction.
/// `None` when nothing matched.
///
/// Several rows matching is **not** ambiguity to decline: an EPUB and the
/// audiobook nothing ever attached to it are one work in two rows, and the
/// scan flow is supervised — `CloseMatch` is a confirmation screen, never
/// auto-applied, so a wrong suggestion costs a tap. (The unattended path,
/// `normalize`/`sync::attach`, keeps its strict guard for the opposite reason:
/// there, a false positive silently corrupts a book.)
///
/// Matches on the **effective** norm — `COALESCE(mo.title_norm, b.title_norm)`
/// — so a user's title/author edit (which lives only in `metadata_overrides`, a
/// read-time overlay; `books.*_norm` keep the scanned values) is what's compared
/// (#checkin-match-effective-title). A physical copy's barcode ISBN is a
/// different edition from the ebook's, so this norm rung — not the exact-ISBN
/// rung — is the only bridge between a scanned physical copy and an existing
/// ebook.
///
/// Two passes:
/// 1. **Exact** effective-norm equality on both title and author.
/// 2. **Subtitle-tolerant** fallback (only when the exact pass found nothing):
///    author still exact, but one effective title may be a word-boundary prefix
///    of the other — so `"the name of the wind"` matches
///    `"the name of the wind the kingkiller chronicle book 1"`.
async fn find_book_by_norm(
    pool: &SqlitePool,
    meta: &ExternalBookMeta,
) -> Result<Option<(ScanBook, Vec<ScanBook>)>, sqlx::Error> {
    let (Some(title_norm), Some(author_norm)) = (
        normalize_title(&meta.title),
        meta.authors.first().and_then(|a| normalize_author(a)),
    ) else {
        return Ok(None);
    };

    let mut candidates = query_norm_candidates(pool, &title_norm, &author_norm, false).await?;
    if candidates.is_empty() {
        candidates = query_norm_candidates(pool, &title_norm, &author_norm, true).await?;
    }
    let mut candidates = candidates.into_iter();
    Ok(candidates.next().map(|first| (first, candidates.collect())))
}

/// Run one pass of the norm rung, returning every matching book up to
/// [`MAX_CLOSE_MATCH_CANDIDATES`]. `tolerant` widens the title predicate from
/// exact equality to word-boundary prefix in either direction; author stays an
/// exact effective-norm match in both modes.
///
/// Two-step lookup (#1343): every book falls into exactly one of two disjoint
/// arms, so their `UNION ALL` is exact.
///
/// 1. **No override row** — the common case. The effective norm *is* the
///    scanned norm here, so this arm compares `b.title_norm`/`b.author_norm`
///    directly, servable by `idx_books_norm` (an index seek, not a `books`
///    scan).
/// 2. **Has an override row** — rare. `metadata_overrides` is small, so
///    joining it and comparing the `COALESCE`d effective norm costs a small
///    table's worth of work rather than a full `books` scan.
async fn query_norm_candidates(
    pool: &SqlitePool,
    title_norm: &str,
    author_norm: &str,
    tolerant: bool,
) -> Result<Vec<ScanBook>, sqlx::Error> {
    // Norm strings are `[a-z0-9 ]` only (see `normalize`), so `?1` carries no
    // LIKE wildcards and the ` %` suffix asserts a word boundary — a prefix
    // match can't span mid-word. The predicates are static strings, not user
    // input, so interpolating them into the SQL is injection-safe.
    let (title_pred_books, title_pred_effective) = if tolerant {
        (
            "(b.title_norm = ?1
              OR b.title_norm LIKE ?1 || ' %'
              OR ?1 LIKE b.title_norm || ' %')",
            "(COALESCE(mo.title_norm, b.title_norm) = ?1
              OR COALESCE(mo.title_norm, b.title_norm) LIKE ?1 || ' %'
              OR ?1 LIKE COALESCE(mo.title_norm, b.title_norm) || ' %')",
        )
    } else {
        (
            "b.title_norm = ?1",
            "COALESCE(mo.title_norm, b.title_norm) = ?1",
        )
    };
    let candidate_cols = "b.uuid, b.title, b.has_cover,
                (SELECT group_concat(a.name, ', ')
                   FROM books_authors_link bal JOIN authors a ON a.id = bal.author
                  WHERE bal.book = b.id ORDER BY bal.position) AS authors,
                EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid) AS has_physical,
                (SELECT REPLACE(REPLACE(bi2.value, '-', ''), ' ', '')
                   FROM book_identifiers bi2
                  WHERE bi2.book_id = b.id AND bi2.scheme LIKE '%isbn%'
                    AND REPLACE(REPLACE(bi2.value, '-', ''), ' ', '')
                        GLOB '[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
                  ORDER BY bi2.rowid LIMIT 1) AS isbn";
    let sql = format!(
        "SELECT {candidate_cols}
           FROM books b
          WHERE {title_pred_books} AND b.author_norm = ?2
            AND NOT EXISTS (SELECT 1 FROM metadata_overrides mo WHERE mo.book_uuid = b.uuid)
         UNION ALL
         SELECT {candidate_cols}
           FROM books b
           JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
          WHERE {title_pred_effective} AND COALESCE(mo.author_norm, b.author_norm) = ?2
          ORDER BY uuid LIMIT {MAX_CLOSE_MATCH_CANDIDATES}"
    );
    let rows = sqlx::query(&sql)
        .bind(title_norm)
        .bind(author_norm)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let isbn = r.get::<Option<String>, _>("isbn").filter(|s| !s.is_empty());
            row_to_scan_book(r, isbn)
        })
        .collect())
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
