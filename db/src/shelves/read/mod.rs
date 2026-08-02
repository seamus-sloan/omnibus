//! Shelf read paths: visibility-scoped listing, single-shelf detail, the
//! member book page, and the create-modal rule preview. Split by read-shape:
//! [`summary`] covers the multi-shelf rail ([`list_visible_shelves`] and its
//! per-kind batch-loaders); [`detail`] covers single-shelf fetches (a shelf's
//! own page, the rule preview, Kobo sync membership). Shared constants and
//! small cross-cutting helpers (rule parsing, sort SQL) live here.

use omnibus_shared::{
    MatchMode, RuleField, RuleOp, ShelfKind, ShelfRule, SortDir, SortKey, Visibility,
};
use sqlx::{Row, SqlitePool};

use super::rules::{membership_predicate, Bind};
use super::ShelfError;

mod detail;
mod summary;

pub use detail::{
    get_shelf, kobo_synced_book_uuids, manual_shelves_containing, preview_rule, shelf_page,
};
pub use summary::list_visible_shelves;

/// Strict downloadable gate for the Kobo sync uuid paths: the book must still
/// have a file on disk — an entitlement the device can't then download is
/// worse than an absent one.
const FILE_EXISTS: &str = "EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)";

/// Visibility gate for the hydrated smart-shelf reads: file-backed, or
/// physical-only via a checked-in copy — mirrors the landing read path's rule
/// (`list_books` in `db/src/books/list.rs`). A ghosted book (file removed, no
/// copy) stays hidden.
const SMART_VISIBLE: &str = "(EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id) \
     OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid))";

/// Cover count in the create-modal live preview.
const PREVIEW_SAMPLE: i64 = 12;

/// A book that can actually render a thumbnail: an embedded cover or an
/// uploaded override. Mosaic tiles filter on this so the gallery never
/// composes a collage cell that 404s.
const HAS_COVER: &str = "(b.has_cover = 1 OR EXISTS (SELECT 1 FROM metadata_overrides mo \
     WHERE mo.book_uuid = b.uuid AND mo.has_cover_override = 1))";

/// Mosaic size: how many member cover uuids `ShelfSummary.cover_uuids` carries.
const MOSAIC_COVERS: i64 = 4;

/// Hard cap on how many shelves `list_visible_shelves` returns for a single
/// viewer. Matches `LIST_BOOKMARKS_LIMIT`/`LIST_HIGHLIGHTS_LIMIT` — a
/// defensive ceiling so a user with a pathological shelf count can't produce
/// an unbounded REST response.
pub const LIST_SHELVES_LIMIT: i64 = 500;

// Max concurrent `count_smart` queries `list_visible_shelves` runs at once.
const SMART_COUNT_CONCURRENCY: usize = 8;

/// Parse one `shelf_rules` row (`field`, `op`, `value` columns) into a
/// [`ShelfRule`]. Shared by [`detail::load_rules`] and
/// [`summary::load_rules_batch`].
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

/// Count of books matching a smart shelf's rules. Shared by
/// [`detail::get_shelf`] / [`detail::preview_rule`] (single-shelf) and
/// [`summary::count_smart_fan_out`] (fanned out per visible shelf).
async fn count_smart(
    pool: &SqlitePool,
    owner_id: i64,
    match_mode: MatchMode,
    rules: &[ShelfRule],
) -> Result<i64, ShelfError> {
    let pred = membership_predicate(rules, match_mode, owner_id)?;
    let sql = format!(
        "SELECT COUNT(*) FROM books b WHERE {SMART_VISIBLE} AND {}",
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
