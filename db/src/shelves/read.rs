//! Shelf read paths: visibility-scoped listing, single-shelf detail, the
//! member book page, and the create-modal rule preview.

use std::collections::HashMap;

use futures::future::try_join_all;
use omnibus_shared::{
    EbookMetadata, MatchMode, RuleField, RuleOp, RulePreview, Shelf, ShelfKind, ShelfPage,
    ShelfRule, ShelfSummary, SortDir, SortKey, Visibility,
};
use sqlx::{Row, SqlitePool};

use super::rules::{membership_predicate, Bind};
use super::ShelfError;
use crate::books::{
    backfill_creator_ids, merge_overrides_into_books, row_to_ebook, BOOK_COLUMNS,
    MAX_BOOKS_RETURNED,
};

/// Books hidden from smart shelves unless they still have a file on disk —
/// mirrors the landing read path's fileless filter (F2).
const FILE_EXISTS: &str = "EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)";

/// Cover count in the create-modal live preview.
const PREVIEW_SAMPLE: i64 = 12;

/// Hard cap on how many shelves `list_visible_shelves` returns for a single
/// viewer. Matches `LIST_BOOKMARKS_LIMIT`/`LIST_HIGHLIGHTS_LIMIT` — a
/// defensive ceiling so a user with a pathological shelf count can't produce
/// an unbounded REST response.
pub const LIST_SHELVES_LIMIT: i64 = 500;

// Max concurrent `count_smart` queries `list_visible_shelves` runs at once.
const SMART_COUNT_CONCURRENCY: usize = 8;

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

/// Every shelf `viewer_id` can see: own + public, or all when `is_admin`.
pub async fn list_visible_shelves(
    pool: &SqlitePool,
    viewer_id: i64,
    is_admin: bool,
) -> Result<Vec<ShelfSummary>, ShelfError> {
    let rows = sqlx::query(
        "SELECT s.id, s.owner_user_id, u.username AS owner_username,
                s.kind, s.name, s.visibility, s.accent, s.match_mode
           FROM shelves s
           JOIN users u ON u.id = s.owner_user_id
          WHERE s.owner_user_id = ? OR s.visibility = 'public' OR ?
          ORDER BY s.position, s.id
          LIMIT ?",
    )
    .bind(viewer_id)
    .bind(is_admin)
    .bind(LIST_SHELVES_LIMIT)
    .fetch_all(pool)
    .await?;

    struct VisibleShelfRow {
        id: i64,
        owner_user_id: i64,
        owner_username: String,
        kind: ShelfKind,
        name: String,
        visibility: Visibility,
        accent: Option<String>,
        match_mode: Option<String>,
    }
    let mut parsed = Vec::with_capacity(rows.len());
    let mut smart_ids = Vec::new();
    let mut manual_ids = Vec::new();
    let mut wishlist_owner_ids = Vec::new();
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let owner_user_id: i64 = r.try_get("owner_user_id")?;
        let kind = parse_kind(&r.try_get::<String, _>("kind")?)?;
        match kind {
            ShelfKind::Smart => smart_ids.push(id),
            ShelfKind::Manual => manual_ids.push(id),
            // Wishlist counts come from `wishlist_entries` keyed by owner, batched
            // below (one GROUP BY) rather than a query per visible wishlist.
            ShelfKind::Wishlist => wishlist_owner_ids.push(owner_user_id),
        }
        parsed.push(VisibleShelfRow {
            id,
            owner_user_id: r.try_get("owner_user_id")?,
            owner_username: r.try_get("owner_username")?,
            kind,
            name: r.try_get("name")?,
            visibility: parse_visibility(&r.try_get::<String, _>("visibility")?)?,
            accent: r.try_get("accent")?,
            match_mode: r.try_get("match_mode")?,
        });
    }

    let mut rules_by_shelf = load_rules_batch(pool, &smart_ids).await?;
    let manual_counts = count_manual_batch(pool, &manual_ids).await?;
    let wishlist_counts = count_wishlist_batch(pool, &wishlist_owner_ids).await?;

    // Each smart shelf's membership predicate is unique, so unlike the manual
    // counts above these can't fold into one `GROUP BY` — fan them out
    // concurrently instead of one sequential query per shelf.
    let mut smart_inputs = Vec::with_capacity(smart_ids.len());
    for row in &parsed {
        if row.kind == ShelfKind::Smart {
            let mode = parse_mode(row.match_mode.clone());
            let rules = rules_by_shelf.remove(&row.id).unwrap_or_default();
            smart_inputs.push((row.id, row.owner_user_id, mode, rules));
        }
    }
    let smart_counts = count_smart_fan_out(pool, smart_inputs).await?;

    let mut out = Vec::with_capacity(parsed.len());
    for row in parsed {
        let book_count = match row.kind {
            ShelfKind::Smart => smart_counts.get(&row.id).copied().unwrap_or(0),
            ShelfKind::Manual => manual_counts.get(&row.id).copied().unwrap_or(0),
            ShelfKind::Wishlist => wishlist_counts
                .get(&row.owner_user_id)
                .copied()
                .unwrap_or(0),
        };
        out.push(ShelfSummary {
            id: row.id,
            owner_user_id: row.owner_user_id,
            owner_username: row.owner_username,
            kind: row.kind,
            name: row.name,
            visibility: row.visibility,
            accent: row.accent,
            book_count,
        });
    }
    Ok(out)
}

/// Full shelf detail (including its rules), or `None` if the id is unknown.
pub async fn get_shelf(pool: &SqlitePool, id: i64) -> Result<Option<Shelf>, ShelfError> {
    let Some(r) = sqlx::query(
        "SELECT s.id, s.owner_user_id, u.username AS owner_username,
                s.kind, s.name, s.description, s.visibility, s.accent, s.match_mode
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
    }))
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
    let total: i64 =
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM books b WHERE {FILE_EXISTS}"))
            .fetch_one(pool)
            .await?;
    let sample = fetch_smart(
        pool,
        owner_id,
        match_mode,
        rules,
        &order_by_sql(SortKey::NewestAdded, SortDir::Desc),
        PREVIEW_SAMPLE,
    )
    .await?;
    Ok(RulePreview {
        matched,
        total,
        sample,
    })
}

// --- helpers ---------------------------------------------------------------

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

/// Load rules for every shelf id in `shelf_ids` in one query, keyed by shelf
/// id in the returned map. Each shelf's own `Vec<ShelfRule>` preserves stored
/// order (`ORDER BY ... position, id`); the map itself has no iteration
/// order. Avoids the per-row `load_rules` call `list_visible_shelves` used to
/// make for each smart shelf.
async fn load_rules_batch(
    pool: &SqlitePool,
    shelf_ids: &[i64],
) -> Result<HashMap<i64, Vec<ShelfRule>>, ShelfError> {
    let mut out: HashMap<i64, Vec<ShelfRule>> = HashMap::new();
    if shelf_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = shelf_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT shelf_id, field, op, value FROM shelf_rules \
         WHERE shelf_id IN ({placeholders}) ORDER BY shelf_id, position, id"
    );
    let mut q = sqlx::query(&sql);
    for id in shelf_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    for r in &rows {
        let shelf_id: i64 = r.try_get("shelf_id")?;
        out.entry(shelf_id).or_default().push(row_to_rule(r)?);
    }
    Ok(out)
}

/// Count manual-shelf membership for every shelf id in `shelf_ids` in one
/// `GROUP BY` query — avoids the per-row `count_manual` call
/// `list_visible_shelves` used to make for each manual shelf. A shelf with
/// zero books has no row in `shelf_books` and so is absent from the map;
/// callers default missing ids to 0.
async fn count_manual_batch(
    pool: &SqlitePool,
    shelf_ids: &[i64],
) -> Result<HashMap<i64, i64>, ShelfError> {
    let mut out = HashMap::new();
    if shelf_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = shelf_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT shelf_id, COUNT(*) AS cnt FROM shelf_books \
         WHERE shelf_id IN ({placeholders}) GROUP BY shelf_id"
    );
    let mut q = sqlx::query(&sql);
    for id in shelf_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    for r in &rows {
        out.insert(r.try_get("shelf_id")?, r.try_get("cnt")?);
    }
    Ok(out)
}

/// Parse one `shelf_rules` row (`field`, `op`, `value` columns) into a
/// [`ShelfRule`]. Shared by [`load_rules`] and [`load_rules_batch`].
fn row_to_rule(r: &sqlx::sqlite::SqliteRow) -> Result<ShelfRule, ShelfError> {
    let field: String = r.try_get("field")?;
    let op: String = r.try_get("op")?;
    Ok(ShelfRule {
        field: RuleField::from_str(&field)
            .ok_or_else(|| ShelfError::InvalidRule(format!("unknown field {field:?}")))?,
        op: RuleOp::from_str(&op)
            .ok_or_else(|| ShelfError::InvalidRule(format!("unknown op {op:?}")))?,
        value: r.try_get("value")?,
    })
}

async fn count_smart(
    pool: &SqlitePool,
    owner_id: i64,
    match_mode: MatchMode,
    rules: &[ShelfRule],
) -> Result<i64, ShelfError> {
    let pred = membership_predicate(rules, match_mode, owner_id)?;
    let sql = format!(
        "SELECT COUNT(*) FROM books b WHERE {FILE_EXISTS} AND {}",
        pred.sql
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for b in &pred.binds {
        q = match b {
            Bind::Text(s) => q.bind(s.clone()),
            Bind::Int(i) => q.bind(*i),
        };
    }
    Ok(q.fetch_one(pool).await?)
}

/// Run [`count_smart`] for every `(shelf_id, owner_id, match_mode, rules)`
/// tuple in `inputs`, fanning out [`SMART_COUNT_CONCURRENCY`] at a time,
/// keyed by shelf id in the returned map. Replaces the sequential per-shelf
/// await `list_visible_shelves` used to make (see its doc comment).
async fn count_smart_fan_out(
    pool: &SqlitePool,
    inputs: Vec<(i64, i64, MatchMode, Vec<ShelfRule>)>,
) -> Result<HashMap<i64, i64>, ShelfError> {
    let mut out = HashMap::with_capacity(inputs.len());
    for chunk in inputs.chunks(SMART_COUNT_CONCURRENCY) {
        // `try_join_all` (not `join_all`) so one failing count short-circuits
        // the rest of the chunk instead of waiting out every in-flight query.
        let counts = try_join_all(
            chunk
                .iter()
                .map(|(_, owner_id, mode, rules)| count_smart(pool, *owner_id, *mode, rules)),
        )
        .await?;
        for ((shelf_id, ..), count) in chunk.iter().zip(counts) {
            out.insert(*shelf_id, count);
        }
    }
    Ok(out)
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

/// Batched wishlist counts keyed by owner id, in one `GROUP BY we.user_id` pass
/// — the [`count_wishlist`] analogue for [`list_visible_shelves`], so a rail
/// showing many public wishlists isn't one query per shelf. An owner with an
/// empty wishlist has no row; callers default a missing id to 0.
async fn count_wishlist_batch(
    pool: &SqlitePool,
    owner_ids: &[i64],
) -> Result<HashMap<i64, i64>, ShelfError> {
    let mut out = HashMap::new();
    if owner_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = owner_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT we.user_id AS uid, COUNT(*) AS cnt FROM wishlist_entries we \
           JOIN books b ON b.uuid = we.book_uuid \
          WHERE we.user_id IN ({placeholders}) GROUP BY we.user_id"
    );
    let mut q = sqlx::query(&sql);
    for id in owner_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    for r in &rows {
        out.insert(r.try_get("uid")?, r.try_get("cnt")?);
    }
    Ok(out)
}

/// One page of the owner's wishlist, newest-added first. Deliberately **omits**
/// the `FILE_EXISTS` gate the smart/manual reads apply: a wishlist-only
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
         WHERE {FILE_EXISTS} AND {} ORDER BY {order_by} LIMIT ?",
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

/// `ORDER BY` for a smart shelf's sort axis. Column set mirrors the landing
/// keyset axes (migration 0028); the axis names are a fixed vocabulary, never
/// user text, so interpolation is safe.
fn order_by_sql(sort: SortKey, dir: SortDir) -> String {
    let d = if dir == SortDir::Asc { "ASC" } else { "DESC" };
    match sort {
        SortKey::Title => format!("b.sort {d}, b.id {d}"),
        SortKey::Author => format!("b.author_sort {d}, b.id {d}"),
        SortKey::Series => format!("b.series_sort {d}, b.series_index {d}, b.id {d}"),
        SortKey::LastUpdated => format!("b.last_modified {d}, b.id {d}"),
        SortKey::NewestAdded => format!("b.timestamp {d}, b.id {d}"),
    }
}

fn parse_kind(s: &str) -> Result<ShelfKind, ShelfError> {
    ShelfKind::from_str(s).ok_or_else(|| ShelfError::InvalidRule(format!("unknown kind {s:?}")))
}

fn parse_visibility(s: &str) -> Result<Visibility, ShelfError> {
    Visibility::from_str(s)
        .ok_or_else(|| ShelfError::InvalidRule(format!("unknown visibility {s:?}")))
}

fn parse_mode(s: Option<String>) -> MatchMode {
    s.as_deref()
        .and_then(MatchMode::from_str)
        .unwrap_or(MatchMode::Any)
}
