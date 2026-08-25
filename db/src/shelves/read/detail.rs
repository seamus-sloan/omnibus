//! Single-shelf reads: a shelf's own detail + member page, the create-modal
//! rule preview, per-book "which shelves" membership, and the Kobo sync
//! membership union. The multi-shelf rail lives in [`super::summary`].

use std::collections::HashSet;

use omnibus_shared::{
    EbookMetadata, MatchMode, RulePreview, Shelf, ShelfKind, ShelfPage, ShelfRule, SortDir, SortKey,
};
use sqlx::{Row, SqlitePool};

use super::{
    count_smart, order_by_sql, parse_kind, parse_visibility, row_to_rule, ShelfError, FILE_EXISTS,
    LIST_SHELVES_LIMIT, PREVIEW_SAMPLE, SMART_VISIBLE,
};
use crate::books::{
    backfill_creator_ids, merge_overrides_into_books, row_to_ebook, BOOK_COLUMNS,
    MAX_BOOKS_RETURNED,
};
use crate::shelves::rules::{membership_predicate, Bind};

/// Ids of the hand-picked shelves `viewer_id` can see that hold `uuid`.
///
/// The membership answer for one book in one request. Without it a client has
/// to fetch every visible shelf's page and scan them — one request per shelf,
/// on every book it displays — which is what the mobile book screen was doing.
///
/// Smart shelves are excluded because their membership is derived from a rule
/// and cannot be toggled; the only caller is a "which shelves is this on"
/// checklist. Returns an empty vec for an unknown uuid rather than erroring —
/// a book the server has never indexed is on no shelf, which is the same
/// answer.
pub async fn manual_shelves_containing(
    pool: &SqlitePool,
    viewer_id: i64,
    is_admin: bool,
    uuid: &str,
) -> Result<Vec<i64>, ShelfError> {
    let Some(canonical) = crate::resolve_canonical_book_uuid(pool, uuid).await? else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query(
        "SELECT s.id
           FROM shelves s
           JOIN shelf_books sb ON sb.shelf_id = s.id
          WHERE sb.book_uuid = ?
            AND s.kind = 'manual'
            AND (s.owner_user_id = ? OR s.visibility = 'public' OR ?)
          ORDER BY s.position, s.id
          LIMIT ?",
    )
    .bind(&canonical)
    .bind(viewer_id)
    .bind(is_admin)
    .bind(LIST_SHELVES_LIMIT)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| r.try_get::<i64, _>("id").map_err(ShelfError::from))
        .collect()
}

/// Book uuids in `candidates` that are confined to a manual shelf `viewer_id`
/// cannot see: on at least one manual shelf, and every one of those shelves
/// is private and owned by someone else. A book on no shelf at all, or on
/// any shelf the viewer owns, that's public, or `is_admin` covers, is never
/// in the result — so this only ever *narrows* an already-visible set.
///
/// This is the predicate the OPDS catalogs layer on top of their normal
/// library reads (#932): a book that's reachable only by hand-picking it
/// onto a private shelf must not surface in a browse/search/new/nav feed —
/// or a direct cover/file link — for a viewer who can't see that shelf,
/// even though the file itself is otherwise an ordinary, generally-served
/// library book. Chunked like [`manual_shelves_containing`]'s siblings so a
/// large candidate page stays under SQLite's bound-parameter cap.
pub async fn shelf_exclusive_hidden_uuids(
    pool: &SqlitePool,
    viewer_id: i64,
    is_admin: bool,
    candidates: &[String],
) -> Result<HashSet<String>, ShelfError> {
    const CHUNK_SIZE: usize = 200;
    let mut hidden = HashSet::new();
    for chunk in candidates.chunks(CHUNK_SIZE) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT sb.book_uuid AS uuid,
                    MAX(CASE WHEN s.owner_user_id = ? OR s.visibility = 'public' OR ?
                             THEN 1 ELSE 0 END) AS any_visible
               FROM shelf_books sb
               JOIN shelves s ON s.id = sb.shelf_id
              WHERE sb.book_uuid IN ({placeholders})
                AND s.kind = 'manual'
              GROUP BY sb.book_uuid"
        );
        let mut q = sqlx::query(&sql).bind(viewer_id).bind(is_admin);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let rows = q.fetch_all(pool).await?;
        for r in &rows {
            let any_visible: i64 = r.try_get("any_visible")?;
            if any_visible == 0 {
                hidden.insert(r.try_get::<String, _>("uuid")?);
            }
        }
    }
    Ok(hidden)
}

/// Full shelf detail (including its rules), or `None` if the id is unknown.
pub async fn get_shelf(pool: &SqlitePool, id: i64) -> Result<Option<Shelf>, ShelfError> {
    let Some(r) = sqlx::query(
        "SELECT s.id, s.owner_user_id, COALESCE(u.display_name, u.username) AS owner_username,
                s.kind, s.name, s.description, s.visibility, s.accent, s.match_mode,
                s.sync_to_kobo
           FROM shelves s
           JOIN users u ON u.id = s.owner_user_id
          WHERE s.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let owner_user_id: i64 = r.try_get("owner_user_id")?;
    let kind = parse_kind(&r.try_get::<String, _>("kind")?)?;
    let match_mode = r
        .try_get::<Option<String>, _>("match_mode")?
        .as_deref()
        .and_then(MatchMode::from_str);
    let rules = match kind {
        ShelfKind::Smart => load_rules(pool, id).await?,
        ShelfKind::Manual | ShelfKind::Wishlist => Vec::new(),
    };
    let book_count = match kind {
        ShelfKind::Smart => {
            count_smart(
                pool,
                owner_user_id,
                match_mode.unwrap_or(MatchMode::Any),
                &rules,
            )
            .await?
        }
        ShelfKind::Manual => count_manual(pool, id).await?,
        ShelfKind::Wishlist => count_wishlist(pool, owner_user_id).await?,
    };

    Ok(Some(Shelf {
        id,
        owner_user_id,
        owner_username: r.try_get("owner_username")?,
        kind,
        name: r.try_get("name")?,
        description: r.try_get("description")?,
        visibility: parse_visibility(&r.try_get::<String, _>("visibility")?)?,
        accent: r.try_get("accent")?,
        match_mode,
        rules,
        book_count,
        sync_to_kobo: r.try_get::<i64, _>("sync_to_kobo")? != 0,
    }))
}

/// Every book uuid the owner's Kobo devices may sync: the union of membership
/// across `user_id`'s shelves flagged `sync_to_kobo`.
///
/// Deliberately **uncapped** — the Kobo sync response streams and must not
/// inherit a page limit (the whole point of #922's no-`SYNC_ITEM_LIMIT` rule),
/// so this does not go through `shelf_page`/`MAX_BOOKS_RETURNED`. Scoped to
/// shelves the user owns, so one user's opt-in can never expose books through
/// another user's device token.
///
/// Returns uuids in no meaningful order; the caller orders the book rows.
pub async fn kobo_synced_book_uuids(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<String>, ShelfError> {
    let shelves = sqlx::query(
        "SELECT id, kind, match_mode FROM shelves
          WHERE owner_user_id = ? AND sync_to_kobo = 1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut uuids: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in &shelves {
        let id: i64 = row.try_get("id")?;
        let kind = parse_kind(&row.try_get::<String, _>("kind")?)?;
        let match_mode = row
            .try_get::<Option<String>, _>("match_mode")?
            .as_deref()
            .and_then(MatchMode::from_str);
        let found = match kind {
            ShelfKind::Manual => manual_member_uuids(pool, id).await?,
            ShelfKind::Wishlist => wishlist_member_uuids(pool, user_id).await?,
            ShelfKind::Smart => {
                let rules = load_rules(pool, id).await?;
                smart_member_uuids(pool, user_id, match_mode.unwrap_or(MatchMode::Any), &rules)
                    .await?
            }
        };
        for uuid in found {
            if seen.insert(uuid.clone()) {
                uuids.push(uuid);
            }
        }
    }
    Ok(uuids)
}

/// One page of a shelf's books (v1: capped at [`MAX_BOOKS_RETURNED`], no
/// cursor). Smart shelves honor `sort`/`dir`; manual shelves ignore them and
/// return `shelf_books.position` order.
pub async fn shelf_page(
    pool: &SqlitePool,
    shelf: &Shelf,
    sort: SortKey,
    dir: SortDir,
) -> Result<ShelfPage, ShelfError> {
    let books = match shelf.kind {
        ShelfKind::Smart => {
            fetch_smart(
                pool,
                shelf.owner_user_id,
                shelf.match_mode.unwrap_or(MatchMode::Any),
                &shelf.rules,
                &order_by_sql(sort, dir),
                MAX_BOOKS_RETURNED,
            )
            .await?
        }
        ShelfKind::Manual => fetch_manual(pool, shelf.id, MAX_BOOKS_RETURNED).await?,
        ShelfKind::Wishlist => {
            fetch_wishlist(pool, shelf.owner_user_id, MAX_BOOKS_RETURNED).await?
        }
    };
    Ok(ShelfPage { books })
}

/// Evaluate an unsaved rule for the create modal: how many of the library match,
/// the library total, and a small cover sample.
pub async fn preview_rule(
    pool: &SqlitePool,
    owner_id: i64,
    match_mode: MatchMode,
    rules: &[ShelfRule],
) -> Result<RulePreview, ShelfError> {
    let matched = count_smart(pool, owner_id, match_mode, rules).await?;
    let total: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM books b WHERE {SMART_VISIBLE}"
    ))
    .fetch_one(pool)
    .await?;
    let sample = fetch_smart(
        pool,
        owner_id,
        match_mode,
        rules,
        &order_by_sql(SortKey::RecentlyInteracted, SortDir::Desc),
        PREVIEW_SAMPLE,
    )
    .await?;
    Ok(RulePreview {
        matched,
        total,
        sample,
    })
}

/// Load a shelf's rules in stored order.
async fn load_rules(pool: &SqlitePool, shelf_id: i64) -> Result<Vec<ShelfRule>, ShelfError> {
    let rows = sqlx::query(
        "SELECT field, op, value FROM shelf_rules WHERE shelf_id = ? ORDER BY position, id",
    )
    .bind(shelf_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_rule).collect()
}

async fn count_manual(pool: &SqlitePool, shelf_id: i64) -> Result<i64, ShelfError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM shelf_books WHERE shelf_id = ?")
            .bind(shelf_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Count the owner's wishlist. Membership is the user's `wishlist_entries`, not
/// `shelf_books` — the join to `books` is what keeps the count consistent with
/// [`fetch_wishlist`], which can only render entries that resolve to a row.
async fn count_wishlist(pool: &SqlitePool, owner_id: i64) -> Result<i64, ShelfError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM wishlist_entries we
           JOIN books b ON b.uuid = we.book_uuid
          WHERE we.user_id = ?",
    )
    .bind(owner_id)
    .fetch_one(pool)
    .await?)
}

/// One page of the owner's wishlist, newest-added first. Deliberately **omits**
/// the visibility gate the smart reads apply: a wishlist-only
/// (fileless) book is hidden from All Books but must appear inside its own
/// wishlist shelf (#1187, AC4).
async fn fetch_wishlist(
    pool: &SqlitePool,
    owner_id: i64,
    limit: i64,
) -> Result<Vec<EbookMetadata>, ShelfError> {
    let sql = format!(
        "SELECT {BOOK_COLUMNS} FROM books b \
         JOIN wishlist_entries we ON we.book_uuid = b.uuid \
         WHERE we.user_id = ? ORDER BY we.added_at DESC, we.id DESC LIMIT ?"
    );
    let rows = sqlx::query(&sql)
        .bind(owner_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    hydrate(pool, &rows).await
}

async fn fetch_smart(
    pool: &SqlitePool,
    owner_id: i64,
    match_mode: MatchMode,
    rules: &[ShelfRule],
    order_by: &str,
    limit: i64,
) -> Result<Vec<EbookMetadata>, ShelfError> {
    let pred = membership_predicate(rules, match_mode, owner_id)?;
    let sql = format!(
        "SELECT {BOOK_COLUMNS} FROM books b \
         WHERE {SMART_VISIBLE} AND {} ORDER BY {order_by} LIMIT ?",
        pred.sql
    );
    let mut q = sqlx::query(&sql);
    for b in &pred.binds {
        q = match b {
            Bind::Text(s) => q.bind(s.clone()),
            Bind::Int(i) => q.bind(*i),
        };
    }
    let rows = q.bind(limit).fetch_all(pool).await?;
    hydrate(pool, &rows).await
}

async fn fetch_manual(
    pool: &SqlitePool,
    shelf_id: i64,
    limit: i64,
) -> Result<Vec<EbookMetadata>, ShelfError> {
    let sql = format!(
        "SELECT {BOOK_COLUMNS} FROM books b \
         JOIN shelf_books sb ON sb.book_uuid = b.uuid \
         WHERE sb.shelf_id = ? ORDER BY sb.position, sb.added_at LIMIT ?"
    );
    let rows = sqlx::query(&sql)
        .bind(shelf_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    hydrate(pool, &rows).await
}

// --- uuid-only membership, for Kobo sync (#924) -----------------------------
//
// These mirror the `fetch_*` helpers above but select only `books.uuid` and
// take no `LIMIT`: the Kobo sync response must not inherit a page cap, and the
// caller wants identity, not hydrated metadata.

/// Hand-picked membership as bare uuids. Fileless books are excluded: a Kobo
/// entitlement the device can't then download is worse than an absent one.
async fn manual_member_uuids(pool: &SqlitePool, shelf_id: i64) -> Result<Vec<String>, ShelfError> {
    let rows = sqlx::query(&format!(
        "SELECT sb.book_uuid FROM shelf_books sb
           JOIN books b ON b.uuid = sb.book_uuid
          WHERE sb.shelf_id = ? AND {FILE_EXISTS}
          ORDER BY sb.position, sb.added_at"
    ))
    .bind(shelf_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|r| Ok(r.try_get("book_uuid")?)).collect()
}

/// Wishlist membership as bare uuids. Unreachable in practice — `update_shelf`
/// rejects every edit to a system shelf, so the flag can't be set on one — but
/// covered rather than left to panic if that ever loosens.
async fn wishlist_member_uuids(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<Vec<String>, ShelfError> {
    let rows = sqlx::query(&format!(
        "SELECT we.book_uuid FROM wishlist_entries we
           JOIN books b ON b.uuid = we.book_uuid
          WHERE we.user_id = ? AND {FILE_EXISTS}
          ORDER BY we.added_at DESC, we.id DESC"
    ))
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|r| Ok(r.try_get("book_uuid")?)).collect()
}

/// Rule-derived membership as bare uuids. Deliberately stricter than the
/// hydrated smart path's visibility gate: `FILE_EXISTS` only, so neither a
/// ghosted book nor a physical-only one syncs to a device.
async fn smart_member_uuids(
    pool: &SqlitePool,
    owner_id: i64,
    match_mode: MatchMode,
    rules: &[ShelfRule],
) -> Result<Vec<String>, ShelfError> {
    let pred = membership_predicate(rules, match_mode, owner_id)?;
    let sql = format!(
        "SELECT b.uuid FROM books b WHERE {FILE_EXISTS} AND {}",
        pred.sql
    );
    let mut q = sqlx::query(&sql);
    for b in &pred.binds {
        q = match b {
            Bind::Text(s) => q.bind(s.clone()),
            Bind::Int(i) => q.bind(*i),
        };
    }
    let rows = q.fetch_all(pool).await?;
    rows.iter().map(|r| Ok(r.try_get("uuid")?)).collect()
}

/// Decode rows into `EbookMetadata`, then merge overrides + backfill creator ids
/// (same post-processing every books read path runs).
async fn hydrate(
    pool: &SqlitePool,
    rows: &[sqlx::sqlite::SqliteRow],
) -> Result<Vec<EbookMetadata>, ShelfError> {
    let mut books = Vec::with_capacity(rows.len());
    for r in rows {
        books.push(row_to_ebook(r)?);
    }
    merge_overrides_into_books(pool, &mut books).await?;
    backfill_creator_ids(pool, &mut books).await?;
    Ok(books)
}
