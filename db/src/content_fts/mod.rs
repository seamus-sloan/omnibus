//! Full-text index over EPUB chapter text (migration `0087`): the post-scan
//! worker pass that populates `book_content_chapters` / `book_content_fts`,
//! and the bm25-ranked content-search read path. Populated by
//! `worker::Task::BackfillContentFts` after each ebook scan; read by the
//! `/api/search/content` REST handler.

mod extract;

use std::path::PathBuf;

use std::collections::{BTreeSet, HashMap};

use omnibus_shared::{ContentSearchHit, SpoilerFilter};
use sqlx::{Row, SqlitePool};

use crate::helpers::{cap_query_len, library_paths_json, sanitize_fts_query, visible_book_sql};

pub use extract::extract_chapter_texts;

/// Cap on returned content hits — one chapter-level citation list, not a
/// paginated browse surface. Also the ceiling on a caller-supplied `limit`.
const MAX_CONTENT_HITS: i64 = 50;

/// How many books a single search may be scoped to. Each uuid is one bind,
/// and the scope exists to narrow a search to a book or a series — not to
/// express an arbitrary set.
const MAX_SCOPE_BOOKS: usize = 50;

/// Optional narrowing for [`search_content_for_paths`].
#[derive(Debug, Default, Clone)]
pub struct ContentSearchScope {
    /// Restrict to these books. Empty searches the whole library.
    pub book_uuids: Vec<String>,
    /// Hit ceiling, clamped to `1..=MAX_CONTENT_HITS`.
    pub limit: Option<i64>,
}

/// Content-search failure space: the read path touches nothing but the DB,
/// so one transparent variant keeps `sqlx::Error` from crossing the module
/// boundary (rule 02).
#[derive(Debug, thiserror::Error)]
pub enum ContentFtsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Search `book_content_fts` across every configured library path, best
/// bm25 rank first, capped at [`MAX_CONTENT_HITS`].
///
/// `q` runs through the same length cap and token sanitisation as the
/// metadata search ([`sanitize_fts_query`] — no column facets here, the
/// index has one column), so arbitrary user input is safe to pass through;
/// an empty or unusable query yields an empty vec. Each hit cites
/// `(book_uuid, spine_index)` plus a `snippet()` excerpt with matched terms
/// wrapped in `[`…`]`.
pub async fn search_content_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
    scope: &ContentSearchScope,
) -> Result<Vec<ContentSearchHit>, ContentFtsError> {
    if library_paths.is_empty() {
        return Ok(Vec::new());
    }
    let capped = cap_query_len(q);
    let Some(match_expr) = sanitize_fts_query(&capped) else {
        return Ok(Vec::new());
    };
    let limit = scope.limit.unwrap_or(MAX_CONTENT_HITS).clamp(1, MAX_CONTENT_HITS);
    let books: Vec<&String> = scope.book_uuids.iter().take(MAX_SCOPE_BOOKS).collect();
    // Scoping is expressed as a uuid `IN (…)` rather than a title filter so
    // it survives a merge: the caller holds the uuid the listing gave it.
    let scope_sql = if books.is_empty() {
        String::new()
    } else {
        let placeholders = vec!["?"; books.len()].join(", ");
        format!("AND c.book_uuid IN ({placeholders})")
    };
    let visible = visible_book_sql("b", "l", "?");
    let sql = format!(
        r"
        SELECT c.book_uuid,
               c.spine_index,
               COALESCE(NULLIF(b.title, ''), b.scan_key) AS title,
               snippet(book_content_fts, 0, '[', ']', '…', 12) AS snip,
               (SELECT ec.title
                  FROM ebook_chapters ec
                 WHERE ec.book_file_id = (
                           SELECT bf.id FROM book_files bf
                            WHERE bf.book_id = b.id
                              AND bf.format = 'EPUB' COLLATE NOCASE
                            ORDER BY bf.ordinal LIMIT 1)
                   AND ec.spine_index <= c.spine_index
                 ORDER BY ec.spine_index DESC, ec.ordinal DESC
                 LIMIT 1)                            AS chapter_title
        FROM book_content_fts
        JOIN book_content_chapters c ON c.id = book_content_fts.rowid
        JOIN books b ON b.uuid = c.book_uuid
        JOIN scan_roots l ON l.id = b.library_id
        WHERE book_content_fts MATCH ?
          AND {visible}
          {scope_sql}
        ORDER BY bm25(book_content_fts), c.book_uuid, c.spine_index
        LIMIT ?
        "
    );
    let mut query = sqlx::query(&sql)
        .bind(&match_expr)
        .bind(library_paths_json(library_paths));
    for uuid in &books {
        query = query.bind(*uuid);
    }
    let rows = query.bind(limit).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| ContentSearchHit {
            book_uuid: r.get("book_uuid"),
            spine_index: r.get("spine_index"),
            title: r.get("title"),
            chapter_title: r.get("chapter_title"),
            snippet: r.get("snip"),
            ahead_of_reader: None,
            position_delta_percent: None,
        })
        .collect())
}

/// Why a query matched nothing, when the query *form* is the likely cause.
///
/// The index ANDs every term, so a natural-language phrase
/// (`"holoCube Cassius whiskey"`) matches nothing while its words each match
/// plenty — and the caller has no way to tell that from an absent corpus.
/// Counting each term separately turns "no results" into "these words are in
/// the book, just never together".
///
/// `None` for a single-term query (where the form cannot be the problem) and
/// when no term matches anything on its own (where the corpus genuinely
/// lacks them, and a hint would mislead).
pub async fn explain_empty_content_search(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
    scope: &ContentSearchScope,
) -> Result<Option<String>, ContentFtsError> {
    let capped = cap_query_len(q);
    let terms: Vec<&str> = capped.split_whitespace().collect();
    if terms.len() < 2 {
        return Ok(None);
    }
    let mut found: Vec<String> = Vec::new();
    for term in &terms {
        let hits = search_content_for_paths(
            pool,
            library_paths,
            term,
            &ContentSearchScope {
                book_uuids: scope.book_uuids.clone(),
                limit: Some(1),
            },
        )
        .await?;
        if !hits.is_empty() {
            found.push((*term).to_string());
        }
    }
    if found.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "This search requires every term to appear in the same chapter. \
         {} of {} terms match on their own ({}) — try one term, or a quoted phrase.",
        found.len(),
        terms.len(),
        found.join(", ")
    )))
}

/// Place every hit against the reader's own position, and apply `filter`.
///
/// Returns how many hits were withheld under
/// [`SpoilerFilter::Exclude`] — the count is the point of that mode: it lets
/// a caller say "there is an answer, but it is ahead of you" without having
/// seen the answer.
///
/// A hit is placed at the **start** of its chapter, because that is the
/// finest granularity the content index records. The comparison is therefore
/// conservative in the direction that matters: a chapter the reader is
/// partway through counts as behind them, and the spoiler risk within a
/// chapter they are already inside is one they took by reading it.
///
/// Both sides are three-valued. A book the reader has no position in, or one
/// whose spine stats have not been extracted, leaves both fields `None` —
/// *cannot tell*, which is not *not a spoiler*. `Exclude` withholds those
/// too, since the whole point of that mode is that an unverified hit must
/// not reach the caller.
pub async fn annotate_spoilers(
    pool: &SqlitePool,
    user_id: i64,
    hits: &mut Vec<ContentSearchHit>,
    filter: SpoilerFilter,
) -> Result<Option<i64>, ContentFtsError> {
    if matches!(filter, SpoilerFilter::None) || hits.is_empty() {
        return Ok(None);
    }
    // One pass per distinct book rather than per hit: a search returning ten
    // hits from one book must not cost ten position reads.
    let uuids: BTreeSet<String> = hits.iter().map(|h| h.book_uuid.clone()).collect();
    let mut reader_percent: HashMap<String, f64> = HashMap::new();
    let mut spine_fractions: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    for uuid in uuids {
        if let Some(percent) = reader_position_percent(pool, user_id, &uuid).await? {
            reader_percent.insert(uuid.clone(), percent);
        }
        spine_fractions.insert(uuid.clone(), chapter_start_percents(pool, &uuid).await?);
    }

    for hit in hits.iter_mut() {
        let (Some(reader), Some(starts)) = (
            reader_percent.get(&hit.book_uuid),
            spine_fractions.get(&hit.book_uuid),
        ) else {
            continue;
        };
        let Some((_, hit_percent)) = starts.iter().find(|(idx, _)| *idx == hit.spine_index) else {
            continue;
        };
        hit.ahead_of_reader = Some(hit_percent > reader);
        hit.position_delta_percent = Some(round1(hit_percent - reader));
    }

    if !matches!(filter, SpoilerFilter::Exclude) {
        return Ok(None);
    }
    let before = hits.len() as i64;
    // `!= Some(false)` and not `== Some(true)`: an unplaceable hit has not
    // been shown to be safe, and this mode's contract is that nothing
    // unverified reaches the caller.
    hits.retain(|h| h.ahead_of_reader == Some(false));
    Ok(Some(before - hits.len() as i64))
}

/// The reader's furthest recorded position in a book, as a whole-book
/// percent. `None` when they have no position there, or when none of their
/// positions can be placed.
async fn reader_position_percent(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<f64>, ContentFtsError> {
    let progress = crate::progress::book_progress(pool, user_id, book_uuid, None)
        .await
        .map_err(|e| match e {
            crate::progress::ProgressError::Sqlx(inner) => ContentFtsError::Db(inner),
            // The uuid came out of a hit row, so the book exists; fold the
            // unreachable variant rather than panicking.
            crate::progress::ProgressError::BookNotFound => {
                ContentFtsError::Db(sqlx::Error::RowNotFound)
            }
        })?;
    Ok(progress
        .records
        .iter()
        .filter_map(|d| d.resolved.percent_through_book)
        .max_by(f64::total_cmp))
}

/// Each spine document's start as a whole-book percent, from the stored
/// spine stats. Empty when the book's structure has not been extracted.
async fn chapter_start_percents(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Vec<(i64, f64)>, ContentFtsError> {
    let Some(book_id) = crate::resolve_book_id_by_uuid(pool, book_uuid)
        .await
        .map_err(books_error)?
    else {
        return Ok(Vec::new());
    };
    let Some((file_id, _)) = crate::book_file_with_id(pool, book_id, "EPUB")
        .await
        .map_err(books_error)?
    else {
        return Ok(Vec::new());
    };
    let stats = crate::epub_structure::get_spine_stats(pool, file_id)
        .await
        .map_err(|e| match e {
            crate::epub_structure::EpubStructureError::Sqlx(inner) => ContentFtsError::Db(inner),
        })?;
    let total: i64 = stats
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(s.visible_chars.max(0)));
    if total <= 0 {
        return Ok(Vec::new());
    }
    Ok(stats
        .iter()
        .map(|s| {
            (
                s.spine_index,
                (s.chars_before.max(0) as f64 / total as f64 * 100.0).clamp(0.0, 100.0),
            )
        })
        .collect())
}

/// Narrow a `BooksError` into this module's error space.
fn books_error(e: crate::books::BooksError) -> ContentFtsError {
    match e {
        crate::books::BooksError::Db(inner) => ContentFtsError::Db(inner),
        crate::books::BooksError::OverridesJson(inner) => {
            ContentFtsError::Db(sqlx::Error::Decode(Box::new(inner)))
        }
        crate::books::BooksError::Other(msg) => ContentFtsError::Db(sqlx::Error::Decode(msg.into())),
    }
}

/// Round to one decimal — a position delta finer than that is noise against
/// a chapter-granular hit placement.
fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// A book whose content index is missing or stale: the lowest-ordinal EPUB
/// file's current stat differs from the stored snapshot (or no rows exist —
/// a freshly Added book looks identical to a stale one here, which is the
/// point: rule 09's derived-validator principle, one statement for both).
struct Candidate {
    book_uuid: String,
    title: String,
    path: PathBuf,
    mtime_epoch: i64,
    size_bytes: i64,
}

/// Every EPUB book under `library_path` needing (re)extraction. The compared
/// file is the book's lowest-ordinal EPUB — the one `book_file_path` serves
/// and the one a reader gets, matching rule 09's "the compared file is the
/// one the server would serve". Non-EPUB books never join and are skipped
/// silently (audiobooks and comics have no extractable text).
async fn fetch_candidates(pool: &SqlitePool, library_path: &str) -> anyhow::Result<Vec<Candidate>> {
    /// `(uuid, title, library root, dir, stem, format, mtime_epoch, size_bytes)`.
    type CandidateRow = (String, String, String, String, String, String, i64, i64);
    let rows: Vec<CandidateRow> = sqlx::query_as(
        "SELECT b.uuid, COALESCE(NULLIF(b.title, ''), b.scan_key), \
                COALESCE(bf.library_path, l.path), COALESCE(bf.path, b.path), \
                bf.filename, bf.format, bf.mtime_epoch, bf.size_bytes \
         FROM books b \
         JOIN scan_roots l ON b.library_id = l.id \
         JOIN book_files bf ON bf.id = ( \
             SELECT id FROM book_files \
             WHERE book_id = b.id AND format = 'EPUB' COLLATE NOCASE \
             ORDER BY ordinal LIMIT 1) \
         WHERE l.path = ? \
           AND NOT EXISTS (SELECT 1 FROM book_content_chapters c \
                           WHERE c.book_uuid = b.uuid \
                             AND c.mtime_epoch = bf.mtime_epoch \
                             AND c.size_bytes = bf.size_bytes) \
         ORDER BY b.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(book_uuid, title, lib, dir, stem, fmt, mtime_epoch, size_bytes)| Candidate {
                book_uuid,
                title,
                path: std::path::Path::new(&lib)
                    .join(&dir)
                    .join(format!("{stem}.{}", fmt.to_lowercase())),
                mtime_epoch,
                size_bytes,
            },
        )
        .collect())
}

/// (Re)index chapter text for every EPUB book under `library_path` whose
/// stored snapshot no longer matches the served file, and prune rows whose
/// uuid no longer resolves to any book (deleted, or merged away — the
/// cascade-free half of the migration's soft-reference choice).
///
/// A Changed file replaces the whole book's rows (delete + reinsert in one
/// transaction), so a shrunk edition can't leave phantom tail chapters. An
/// unreadable file is logged and skipped — retried next scan, mirroring the
/// sibling backfills — and a readable book whose every chapter is navigation
/// or empty stores no rows, so it re-extracts each scan at the cost of one
/// zip open (the `backfill_covers` retry semantics). `on_progress(processed,
/// total, item)` mirrors the sibling backfills.
pub async fn backfill_content_fts(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32, &str),
) -> anyhow::Result<()> {
    // Prune first so a candidate re-index can't resurrect an orphan's rows'
    // uniqueness slots, and so the FTS index stops serving deleted books
    // even on a pass with no candidates.
    sqlx::query(
        "DELETE FROM book_content_chapters WHERE book_uuid NOT IN (SELECT uuid FROM books)",
    )
    .execute(pool)
    .await?;

    let candidates = fetch_candidates(pool, library_path).await?;
    if candidates.is_empty() {
        return Ok(());
    }
    let total = u32::try_from(candidates.len()).unwrap_or(u32::MAX);
    tracing::info!(count = total, "indexing epub content for search");

    let mut processed = 0u32;
    for candidate in candidates {
        processed = processed.saturating_add(1);
        on_progress(processed, total, &candidate.title);
        let path = candidate.path.clone();
        let chapters = tokio::task::spawn_blocking(move || extract_chapter_texts(&path))
            .await
            .unwrap_or_else(|join_err| {
                tracing::warn!(
                    book_uuid = %candidate.book_uuid,
                    %join_err,
                    is_panic = join_err.is_panic(),
                    "content extraction task failed; leaving unindexed"
                );
                None
            });
        let Some(chapters) = chapters else {
            tracing::warn!(
                book_uuid = %candidate.book_uuid,
                path = %candidate.path.display(),
                "content extraction could not read epub; will retry next scan"
            );
            continue;
        };
        replace_book_chapters(pool, &candidate, &chapters)
            .await
            .map_err(|e| anyhow::anyhow!("store content index for {}: {e}", candidate.book_uuid))?;
    }
    Ok(())
}

/// Delete-and-reinsert one book's chapter rows in a single transaction,
/// stamping each row with the snapshot the text was extracted from. The
/// migration's triggers mirror both halves into `book_content_fts`, so the
/// index and the content table move together or not at all.
async fn replace_book_chapters(
    pool: &SqlitePool,
    candidate: &Candidate,
    chapters: &[extract::ChapterText],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM book_content_chapters WHERE book_uuid = ?")
        .bind(&candidate.book_uuid)
        .execute(&mut *tx)
        .await?;
    for chapter in chapters {
        sqlx::query(
            "INSERT INTO book_content_chapters \
                 (book_uuid, spine_index, mtime_epoch, size_bytes, text) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&candidate.book_uuid)
        .bind(chapter.spine_index)
        .bind(candidate.mtime_epoch)
        .bind(candidate.size_bytes)
        .bind(&chapter.text)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests;
