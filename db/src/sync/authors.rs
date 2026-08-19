//! Batched author-link writer for the indexer sync path. Resolves the
//! merged `creators + contributors` list against the `ignored_authors`
//! blocklist and the `entity_aliases` reindex-resurrection guard,
//! upserts surviving unmerged names into `authors`, and writes the
//! per-book join rows in a constant handful of statements.

use std::collections::{HashMap, HashSet};

use omnibus_shared::{CleanupKind, EbookMetadata};
use sqlx::Transaction;

use crate::entity_alias::resolve_entity_aliases;

#[cfg(test)]
mod tests;

/// Batch-insert author join rows from the merged `creators` list. The OPF
/// parser appends `<dc:contributor>` entries onto `creators` in source
/// order (see `db::ebook::parse`), so positions here are simply `0..n` in
/// that combined order. Names on the `ignored_authors` blocklist are
/// skipped — leaving a gap in `position` rather than renumbering — identical
/// to the previous per-author loop, but resolved in a constant handful of
/// statements instead of ~4 per author.
pub(super) async fn insert_author_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    // (name, sort, position) in OPF order: creators + contributors are
    // already merged into `m.creators` by the parser.
    let entries: Vec<(&str, Option<&str>, i64)> = m
        .creators
        .iter()
        .enumerate()
        .map(|(pos, c)| {
            (
                c.name.as_str(),
                c.file_as.as_deref(),
                i64::try_from(pos).unwrap_or(i64::MAX),
            )
        })
        .collect();
    if entries.is_empty() {
        return Ok(());
    }

    let (order, sort_for) = distinct_names_in_order(&entries);
    let ignored = fetch_blocklisted_names(tx, &order).await?;

    let kept: Vec<&str> = order
        .iter()
        .copied()
        .filter(|n| !ignored.contains(*n))
        .collect();
    if kept.is_empty() {
        return Ok(());
    }

    // #964: a name a completed library-cleanup merge absorbed must resolve
    // to the surviving canonical id rather than mint a fresh `authors` row
    // when the same file is reindexed. Only the unmerged remainder goes
    // through the normal upsert.
    let aliased = resolve_entity_aliases(tx, CleanupKind::Author, &kept).await?;
    let to_upsert: Vec<&str> = kept
        .iter()
        .copied()
        .filter(|n| !aliased.contains_key(*n))
        .collect();
    if !to_upsert.is_empty() {
        upsert_authors(tx, &to_upsert, &sort_for).await?;
    }

    insert_books_authors_link(tx, book_id, &entries, &ignored, &aliased).await?;
    Ok(())
}

/// Walk `entries` once and produce (a) the distinct names in first-seen
/// order and (b) a name → first-non-null sort lookup. Mirrors the old
/// per-row `COALESCE(authors.sort, excluded.sort)`.
fn distinct_names_in_order<'a>(
    entries: &[(&'a str, Option<&'a str>, i64)],
) -> (
    Vec<&'a str>,
    std::collections::HashMap<&'a str, Option<&'a str>>,
) {
    let mut order: Vec<&'a str> = Vec::new();
    let mut sort_for: std::collections::HashMap<&'a str, Option<&'a str>> =
        std::collections::HashMap::new();
    for (name, sort, _) in entries {
        if let Some(slot) = sort_for.get_mut(name) {
            if slot.is_none() {
                *slot = *sort;
            }
        } else {
            order.push(name);
            sort_for.insert(name, *sort);
        }
    }
    (order, sort_for)
}

/// Return the subset of `names` that exist in `ignored_authors`. The
/// comparison is `ignored_authors.name = v.column1`, which inherits the
/// stored column's NOCASE collation (matching the old per-name
/// `WHERE name = ?`). Returning the book's own name strings (verbatim,
/// from the VALUES list) keeps the caller's skip check exact.
async fn fetch_blocklisted_names(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    order: &[&str],
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    let name_rows = std::iter::repeat_n("(?)", order.len())
        .collect::<Vec<_>>()
        .join(", ");
    let ignored_sql = format!(
        "SELECT v.column1 FROM (VALUES {name_rows}) AS v \
         WHERE EXISTS (SELECT 1 FROM ignored_authors i WHERE i.name = v.column1)"
    );
    let mut q = sqlx::query_scalar::<_, String>(&ignored_sql);
    for name in order {
        q = q.bind(*name);
    }
    Ok(q.fetch_all(&mut **tx).await?.into_iter().collect())
}

/// Upsert every kept author in one statement. On a name collision (NOCASE
/// unique on `authors.name`) keeps the existing sort if non-null, otherwise
/// takes the new one.
async fn upsert_authors(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    kept: &[&str],
    sort_for: &std::collections::HashMap<&str, Option<&str>>,
) -> Result<(), sqlx::Error> {
    let author_rows = std::iter::repeat_n("(?, ?)", kept.len())
        .collect::<Vec<_>>()
        .join(", ");
    let upsert_sql = format!(
        "INSERT INTO authors (name, sort) VALUES {author_rows} \
         ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)"
    );
    let mut q = sqlx::query(&upsert_sql);
    for name in kept {
        q = q.bind(*name).bind(sort_for[*name]);
    }
    q.execute(&mut **tx).await?;
    Ok(())
}

/// Insert the per-book link rows. Dedupes by name, keeping the first
/// (lowest) position so a name repeated in the merged creators+contributors
/// list keeps its first position — matching the old `INSERT OR IGNORE`
/// loop. Splits on whether `aliased` already knows the target id (#964: a
/// merged-away name) or needs the usual NOCASE join against `authors`.
async fn insert_books_authors_link(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    entries: &[(&str, Option<&str>, i64)],
    ignored: &HashSet<String>,
    aliased: &HashMap<String, i64>,
) -> Result<(), sqlx::Error> {
    let mut linked: HashSet<&str> = HashSet::new();
    let link_entries: Vec<(&str, i64)> = entries
        .iter()
        .filter(|(name, _, _)| !ignored.contains(*name))
        .filter(|(name, _, _)| linked.insert(name))
        .map(|(name, _, pos)| (*name, *pos))
        .collect();
    if link_entries.is_empty() {
        return Ok(());
    }
    let (direct, by_name): (Vec<_>, Vec<_>) = link_entries
        .into_iter()
        .partition(|(name, _)| aliased.contains_key(*name));

    if !direct.is_empty() {
        insert_direct_author_links(tx, book_id, &direct, aliased).await?;
    }
    if !by_name.is_empty() {
        insert_named_author_links(tx, book_id, &by_name).await?;
    }
    Ok(())
}

/// Link rows whose author id is already known (a merged-away name resolved
/// via `entity_aliases`) — a plain `VALUES` insert, no join needed.
async fn insert_direct_author_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    entries: &[(&str, i64)],
    aliased: &HashMap<String, i64>,
) -> Result<(), sqlx::Error> {
    // `.get()`, not indexing, so a name the caller's filter missed is dropped rather than panicking.
    let resolved: Vec<(i64, i64)> = entries
        .iter()
        .filter_map(|(name, pos)| aliased.get(*name).map(|&author_id| (author_id, *pos)))
        .collect();
    if resolved.is_empty() {
        return Ok(());
    }
    let rows = std::iter::repeat_n("(?, ?, ?)", resolved.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql =
        format!("INSERT OR IGNORE INTO books_authors_link (book, author, position) VALUES {rows}");
    let mut q = sqlx::query(&sql);
    for (author_id, pos) in resolved {
        q = q.bind(book_id).bind(author_id).bind(pos);
    }
    q.execute(&mut **tx).await?;
    Ok(())
}

/// Link rows resolved by name via the NOCASE join against `authors`, for
/// the (common) case of a name with no alias — unchanged from before #964.
async fn insert_named_author_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    entries: &[(&str, i64)],
) -> Result<(), sqlx::Error> {
    let link_rows = std::iter::repeat_n("(?, ?)", entries.len())
        .collect::<Vec<_>>()
        .join(", ");
    let link_sql = format!(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position) \
         SELECT ?, a.id, v.column2 \
         FROM (VALUES {link_rows}) AS v \
         JOIN authors a ON a.name = v.column1"
    );
    let mut q = sqlx::query(&link_sql).bind(book_id);
    for (name, pos) in entries {
        q = q.bind(*name).bind(*pos);
    }
    q.execute(&mut **tx).await?;
    Ok(())
}
