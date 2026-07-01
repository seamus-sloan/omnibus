//! Shelf read paths: visibility-scoped listing, single-shelf detail, the
//! member book page, and the create-modal rule preview.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{
    EbookMetadata, MatchMode, RuleField, RuleOp, RulePreview, Shelf, ShelfKind, ShelfPage,
    ShelfRule, ShelfSummary, SortDir, SortKey, Visibility,
};

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

/// Every shelf `viewer_id` can see: own + public, or all when `is_admin`.
/// Each row carries its live book count (smart = rule match, manual = row count).
pub async fn list_visible_shelves(
    pool: &SqlitePool,
    viewer_id: i64,
    is_admin: bool,
) -> Result<Vec<ShelfSummary>, ShelfError> {
    let rows = sqlx::query(
        "SELECT id, owner_user_id, kind, name, visibility, accent, match_mode
           FROM shelves
          WHERE owner_user_id = ? OR visibility = 'public' OR ?
          ORDER BY position, id",
    )
    .bind(viewer_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let owner_user_id: i64 = r.try_get("owner_user_id")?;
        let kind = parse_kind(r.try_get("kind")?)?;
        let book_count = match kind {
            ShelfKind::Smart => {
                let mode = parse_mode(r.try_get("match_mode")?);
                let rules = load_rules(pool, id).await?;
                count_smart(pool, owner_user_id, mode, &rules).await?
            }
            ShelfKind::Manual => count_manual(pool, id).await?,
        };
        out.push(ShelfSummary {
            id,
            owner_user_id,
            kind,
            name: r.try_get("name")?,
            visibility: parse_visibility(r.try_get("visibility")?)?,
            accent: r.try_get("accent")?,
            book_count,
        });
    }
    Ok(out)
}

/// Full shelf detail (including its rules), or `None` if the id is unknown.
pub async fn get_shelf(pool: &SqlitePool, id: i64) -> Result<Option<Shelf>, ShelfError> {
    let Some(r) = sqlx::query(
        "SELECT id, owner_user_id, kind, name, description, visibility, accent, match_mode
           FROM shelves WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let owner_user_id: i64 = r.try_get("owner_user_id")?;
    let kind = parse_kind(r.try_get("kind")?)?;
    let match_mode = r
        .try_get::<Option<String>, _>("match_mode")?
        .as_deref()
        .and_then(MatchMode::from_str);
    let rules = match kind {
        ShelfKind::Smart => load_rules(pool, id).await?,
        ShelfKind::Manual => Vec::new(),
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
    };

    Ok(Some(Shelf {
        id,
        owner_user_id,
        kind,
        name: r.try_get("name")?,
        description: r.try_get("description")?,
        visibility: parse_visibility(r.try_get("visibility")?)?,
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
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let field: String = r.try_get("field")?;
        let op: String = r.try_get("op")?;
        out.push(ShelfRule {
            field: RuleField::from_str(&field)
                .ok_or_else(|| ShelfError::InvalidRule(format!("unknown field {field:?}")))?,
            op: RuleOp::from_str(&op)
                .ok_or_else(|| ShelfError::InvalidRule(format!("unknown op {op:?}")))?,
            value: r.try_get("value")?,
        });
    }
    Ok(out)
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

async fn count_manual(pool: &SqlitePool, shelf_id: i64) -> Result<i64, ShelfError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM shelf_books WHERE shelf_id = ?")
            .bind(shelf_id)
            .fetch_one(pool)
            .await?,
    )
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

fn parse_kind(s: String) -> Result<ShelfKind, ShelfError> {
    ShelfKind::from_str(&s).ok_or_else(|| ShelfError::InvalidRule(format!("unknown kind {s:?}")))
}

fn parse_visibility(s: String) -> Result<Visibility, ShelfError> {
    Visibility::from_str(&s)
        .ok_or_else(|| ShelfError::InvalidRule(format!("unknown visibility {s:?}")))
}

fn parse_mode(s: Option<String>) -> MatchMode {
    s.as_deref()
        .and_then(MatchMode::from_str)
        .unwrap_or(MatchMode::Any)
}
