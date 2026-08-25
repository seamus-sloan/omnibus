//! Full-library replica: a background task pages through `GET /api/ebooks`
//! after a successful landing fetch and stores every row locally, so
//! offline browsing covers the whole library — not just pages the user
//! happened to visit. Offline, `data::get_ebooks_page` re-implements
//! sort/filter/pagination over the replica with pure functions here.

use std::sync::atomic::{AtomicBool, Ordering};

use omnibus_shared::{EbookLibrary, EbookMetadata, LibraryPage, SortDir, SortKey, ViewFilters};

use super::{cache, media, store};

/// Don't re-page the whole library more often than this.
const REPLICA_TTL_SECS: i64 = 300;
/// Page size for the background replica sync.
const SYNC_PAGE_LIMIT: i64 = 200;
/// Hard safety cap on replica rows (a runaway cursor loop must terminate).
const REPLICA_ROW_CAP: usize = 50_000;
/// Offline search returns at most this many hits (mirrors server capping).
const SEARCH_CAP: usize = 100;

static SYNCING: AtomicBool = AtomicBool::new(false);

/// Kick a background replica refresh if one isn't already running. Cheap to
/// call on every successful first-page fetch — the TTL check inside skips
/// fresh replicas.
pub(crate) fn ensure_fresh(server_url: String) {
    if SYNCING.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = media::spawn_on_runtime(async move {
        sync_replica(&server_url).await;
        SYNCING.store(false, Ordering::SeqCst);
    });
    if !spawned {
        SYNCING.store(false, Ordering::SeqCst);
    }
}

async fn sync_replica(server_url: &str) {
    let Some(st) = store::store() else { return };
    if let Some(row) = st.kv_get(&cache::keys::ebooks_all()).await {
        if store::now_secs() - row.fetched_at < REPLICA_TTL_SECS {
            return;
        }
    }
    let mut all: Vec<EbookMetadata> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        // Deliberately no exclusion: the mirror holds the FULL library so an
        // offline pref toggle can restore hidden books without a resync.
        let page = crate::data::get_ebooks_page_online(
            server_url,
            SortKey::Title,
            SortDir::Asc,
            ViewFilters::default(),
            Vec::new(),
            cursor,
            SYNC_PAGE_LIMIT,
        )
        .await;
        match page {
            Ok(page) => {
                all.extend(page.books);
                match page.next_cursor {
                    Some(next) if all.len() < REPLICA_ROW_CAP => cursor = Some(next),
                    _ => break,
                }
            }
            // A partial replica would make books "disappear" offline —
            // keep the previous complete snapshot instead.
            Err(_) => return,
        }
    }
    tracing::debug!(count = all.len(), "library replica refreshed");
    cache::put_json(&cache::keys::ebooks_all(), &all);
}

/// Serve one offline page from the replica, or `None` when no replica has
/// been synced yet.
pub(crate) async fn page_from_cache(
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: &ViewFilters,
    exclude_formats: &[String],
    cursor: Option<&str>,
    limit: i64,
) -> Option<LibraryPage> {
    let books: Vec<EbookMetadata> = cache::get_json(&cache::keys::ebooks_all()).await?;
    Some(page_from_replica(
        books,
        sort_key,
        sort_dir,
        &filters.formats,
        exclude_formats,
        cursor,
        limit,
    ))
}

/// Serve offline search results from the replica (title / author / filename
/// substring match), or `None` when no replica exists.
pub(crate) async fn search_from_cache(q: &str) -> Option<EbookLibrary> {
    let books: Vec<EbookMetadata> = cache::get_json(&cache::keys::ebooks_all()).await?;
    let hits = search_replica(&books, q);
    let total = hits.len() as i64;
    Some(EbookLibrary {
        path: None,
        books: hits,
        error: None,
        total: Some(total),
    })
}

/// Pure offline pagination over the replica. Offline cursors are
/// `off:{offset}`; an online keyset cursor left over from a dropped
/// connection is unparseable and ends the stream (an empty final page)
/// rather than restarting from the top and duplicating grid keys.
pub(crate) fn page_from_replica(
    books: Vec<EbookMetadata>,
    sort_key: SortKey,
    sort_dir: SortDir,
    formats: &[String],
    exclude_formats: &[String],
    cursor: Option<&str>,
    limit: i64,
) -> LibraryPage {
    let is_first_page = cursor.is_none();
    let offset = match cursor {
        None => Some(0usize),
        Some(c) => c.strip_prefix("off:").and_then(|n| n.parse::<usize>().ok()),
    };
    let Some(offset) = offset else {
        return LibraryPage {
            path: None,
            books: Vec::new(),
            next_cursor: None,
            total: None,
            facets: None,
            hidden_count: None,
        };
    };

    // The replica always holds the full library; hiding is applied at read
    // time so an offline pref toggle restores hidden books instantly. The
    // receipt is the server's same-filters diff: count under the include
    // chips alone vs. chips + exclusion.
    let chip_matched: Vec<EbookMetadata> = books
        .into_iter()
        .filter(|b| matches_formats(b, formats))
        .collect();
    let unexcluded_total = chip_matched.len();
    let mut filtered: Vec<EbookMetadata> = chip_matched
        .into_iter()
        .filter(|b| visible_under_exclusion(b, exclude_formats))
        .collect();
    sort_books(&mut filtered, sort_key, sort_dir);

    let total = filtered.len();
    let limit = limit.max(1) as usize;
    let end = (offset + limit).min(total);
    let page: Vec<EbookMetadata> = if offset < total {
        filtered[offset..end].to_vec()
    } else {
        Vec::new()
    };
    LibraryPage {
        path: None,
        books: page,
        next_cursor: (end < total).then(|| format!("off:{end}")),
        total: is_first_page.then_some(total as i64),
        facets: None,
        hidden_count: (is_first_page && !exclude_formats.is_empty())
            .then_some((unexcluded_total - total) as i64),
    }
}

/// Whether `book` survives the hidden-formats exclusion: visible while any
/// of its formats is not hidden, and always visible when it has no formats
/// at all or holds a physical copy — mirroring the server predicate, whose
/// physical arm keeps an owned book visible even when every file format is
/// hidden.
pub(crate) fn visible_under_exclusion(book: &EbookMetadata, hidden: &[String]) -> bool {
    if hidden.is_empty() || book.formats.is_empty() || book.has_physical {
        return true;
    }
    book.formats
        .iter()
        .any(|f| !hidden.iter().any(|h| h.eq_ignore_ascii_case(f)))
}

/// Case-insensitive substring search over title / creators / filename.
pub(crate) fn search_replica(books: &[EbookMetadata], q: &str) -> Vec<EbookMetadata> {
    let needle = q.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    books
        .iter()
        .filter(|b| {
            b.title
                .as_deref()
                .is_some_and(|t| t.to_lowercase().contains(&needle))
                || b.filename.to_lowercase().contains(&needle)
                || b.creators
                    .iter()
                    .any(|c| c.name.to_lowercase().contains(&needle))
        })
        .take(SEARCH_CAP)
        .cloned()
        .collect()
}

/// `true` when the book carries any of the selected lowercase format keys
/// (OR within the facet, matching the server's filter semantics). An empty
/// filter matches everything.
fn matches_formats(book: &EbookMetadata, formats: &[String]) -> bool {
    if formats.is_empty() {
        return true;
    }
    book.formats
        .iter()
        .any(|f| formats.iter().any(|want| f.eq_ignore_ascii_case(want)))
}

/// Sort the replica to approximate the server's ordering. `LastUpdated`
/// approximates the DB's `last_modified` with the closest wire field
/// (`added_at` falls back to `modified`); exact parity isn't required
/// offline — stability and sanity are.
fn sort_books(books: &mut [EbookMetadata], sort_key: SortKey, sort_dir: SortDir) {
    books.sort_by(|a, b| {
        let ord = match sort_key {
            SortKey::Title => title_key(a).cmp(&title_key(b)),
            SortKey::Author => author_key(a)
                .cmp(&author_key(b))
                .then_with(|| title_key(a).cmp(&title_key(b))),
            SortKey::Series => series_key(a)
                .cmp(&series_key(b))
                .then_with(|| title_key(a).cmp(&title_key(b))),
            SortKey::LastUpdated | SortKey::NewestAdded => date_key(a)
                .cmp(&date_key(b))
                .then_with(|| title_key(a).cmp(&title_key(b))),
            SortKey::RecentlyInteracted => interacted_key(a)
                .cmp(&interacted_key(b))
                .then_with(|| title_key(a).cmp(&title_key(b))),
        };
        match sort_dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
}

fn title_key(b: &EbookMetadata) -> String {
    b.title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&b.filename)
        .to_lowercase()
}

fn author_key(b: &EbookMetadata) -> String {
    b.creators
        .first()
        .map(|c| c.name.to_lowercase())
        .unwrap_or_default()
}

/// Books without a series sort after every series (matching the server's
/// NULLs-last behavior); the index breaks ties inside one series.
fn series_key(b: &EbookMetadata) -> (bool, String, i64) {
    let name = b.series.as_deref().unwrap_or_default().to_lowercase();
    let index = b
        .series_index
        .as_deref()
        .and_then(|i| i.parse::<f64>().ok())
        .map(|i| (i * 1000.0) as i64)
        .unwrap_or(0);
    (b.series.is_none(), name, index)
}

/// ISO-ish date strings compare lexicographically; empty dates sort first
/// so `Desc` (the common "newest" direction) puts them last.
/// Unlike [`date_key`], this is the server's own axis value rather than an
/// approximation — `last_interacted_at` is projected onto every listing, so
/// the replica reproduces the server ordering exactly.
fn interacted_key(b: &EbookMetadata) -> String {
    b.last_interacted_at.clone().unwrap_or_default()
}

fn date_key(b: &EbookMetadata) -> String {
    b.added_at
        .clone()
        .or_else(|| b.modified.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
