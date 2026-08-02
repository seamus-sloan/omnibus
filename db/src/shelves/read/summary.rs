//! The multi-shelf rail: [`list_visible_shelves`] and its per-kind
//! batch-loaders (counts, mosaic covers, rules), each fetching every visible
//! shelf's data in as few round-trips as the kind allows. Single-shelf reads
//! live in [`super::detail`].

use std::collections::HashMap;

use futures::future::try_join_all;
use omnibus_shared::{MatchMode, ShelfKind, ShelfRule, ShelfSummary, SortDir, SortKey, Visibility};
use sqlx::{Row, SqlitePool};

use super::{
    count_smart, order_by_sql, parse_kind, parse_mode, parse_visibility, row_to_rule, ShelfError,
    HAS_COVER, LIST_SHELVES_LIMIT, MOSAIC_COVERS, SMART_COUNT_CONCURRENCY, SMART_VISIBLE,
};
use crate::shelves::rules::{membership_predicate, Bind};

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

/// Per-kind id groups used to fan the batch loaders out to the right query
/// (smart shelves count via their rule predicate, manual/wishlist via a
/// single `GROUP BY`).
struct ShelfIdGroups {
    smart_ids: Vec<i64>,
    manual_ids: Vec<i64>,
    wishlist_owner_ids: Vec<i64>,
}

/// Every shelf `viewer_id` can see: own + public, or all when `is_admin`.
pub async fn list_visible_shelves(
    pool: &SqlitePool,
    viewer_id: i64,
    is_admin: bool,
) -> Result<Vec<ShelfSummary>, ShelfError> {
    let (parsed, groups) = fetch_visible_shelf_rows(pool, viewer_id, is_admin).await?;
    let (counts, covers) = load_shelf_batches(pool, &parsed, &groups).await?;
    Ok(assemble_shelf_summaries(parsed, counts, covers))
}

/// Run the visibility-scoped shelf query and parse each row into a
/// [`VisibleShelfRow`], grouping ids by kind for the batch loaders that
/// follow.
async fn fetch_visible_shelf_rows(
    pool: &SqlitePool,
    viewer_id: i64,
    is_admin: bool,
) -> Result<(Vec<VisibleShelfRow>, ShelfIdGroups), ShelfError> {
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
    Ok((
        parsed,
        ShelfIdGroups {
            smart_ids,
            manual_ids,
            wishlist_owner_ids,
        },
    ))
}

/// Per-kind book counts, keyed to match how each kind's batch loader groups
/// its rows (shelf id for smart/manual, owner id for wishlist).
struct ShelfCounts {
    smart: HashMap<i64, i64>,
    manual: HashMap<i64, i64>,
    wishlist: HashMap<i64, i64>,
}

/// Per-kind mosaic cover uuids, same keying convention as [`ShelfCounts`].
struct ShelfCovers {
    smart: HashMap<i64, Vec<String>>,
    manual: HashMap<i64, Vec<String>>,
    wishlist: HashMap<i64, Vec<String>>,
}

/// Run every count/cover batch loader, fanning the smart-shelf ones out
/// concurrently since each shelf's membership predicate is unique and can't
/// fold into one `GROUP BY` like manual/wishlist can.
async fn load_shelf_batches(
    pool: &SqlitePool,
    parsed: &[VisibleShelfRow],
    groups: &ShelfIdGroups,
) -> Result<(ShelfCounts, ShelfCovers), ShelfError> {
    let mut rules_by_shelf = load_rules_batch(pool, &groups.smart_ids).await?;
    let manual_counts = count_manual_batch(pool, &groups.manual_ids).await?;
    let wishlist_counts = count_wishlist_batch(pool, &groups.wishlist_owner_ids).await?;
    let manual_covers = covers_manual_batch(pool, &groups.manual_ids).await?;
    let wishlist_covers = covers_wishlist_batch(pool, &groups.wishlist_owner_ids).await?;

    let mut smart_inputs = Vec::with_capacity(groups.smart_ids.len());
    for row in parsed {
        if row.kind == ShelfKind::Smart {
            let mode = parse_mode(row.match_mode.clone());
            let rules = rules_by_shelf.remove(&row.id).unwrap_or_default();
            smart_inputs.push((row.id, row.owner_user_id, mode, rules));
        }
    }
    let smart_covers = covers_smart_fan_out(pool, &smart_inputs).await?;
    let smart_counts = count_smart_fan_out(pool, smart_inputs).await?;

    Ok((
        ShelfCounts {
            smart: smart_counts,
            manual: manual_counts,
            wishlist: wishlist_counts,
        },
        ShelfCovers {
            smart: smart_covers,
            manual: manual_covers,
            wishlist: wishlist_covers,
        },
    ))
}

/// Zip parsed rows with their batch-loaded counts/covers into the wire type.
fn assemble_shelf_summaries(
    parsed: Vec<VisibleShelfRow>,
    counts: ShelfCounts,
    mut covers: ShelfCovers,
) -> Vec<ShelfSummary> {
    let mut out = Vec::with_capacity(parsed.len());
    for row in parsed {
        let book_count = match row.kind {
            ShelfKind::Smart => counts.smart.get(&row.id).copied().unwrap_or(0),
            ShelfKind::Manual => counts.manual.get(&row.id).copied().unwrap_or(0),
            ShelfKind::Wishlist => counts
                .wishlist
                .get(&row.owner_user_id)
                .copied()
                .unwrap_or(0),
        };
        let cover_uuids = match row.kind {
            ShelfKind::Smart => covers.smart.remove(&row.id).unwrap_or_default(),
            ShelfKind::Manual => covers.manual.remove(&row.id).unwrap_or_default(),
            // Two visible rows can share an owner's wishlist covers only if the
            // same user somehow owned two wishlists; `remove` keeps the map
            // drain simple and the first row wins.
            ShelfKind::Wishlist => covers
                .wishlist
                .remove(&row.owner_user_id)
                .unwrap_or_default(),
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
            cover_uuids,
        });
    }
    out
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

/// First-[`MOSAIC_COVERS`] cover-bearing member uuids per manual shelf, in
/// `shelf_books.position` order, one windowed query for the whole rail —
/// the mosaic analogue of [`count_manual_batch`]. Shelves whose members all
/// lack covers are absent; callers default to empty.
async fn covers_manual_batch(
    pool: &SqlitePool,
    shelf_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, ShelfError> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if shelf_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = shelf_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT shelf_id, book_uuid FROM ( \
           SELECT sb.shelf_id, sb.book_uuid, \
                  ROW_NUMBER() OVER (PARTITION BY sb.shelf_id \
                                     ORDER BY sb.position, sb.added_at) AS rn \
             FROM shelf_books sb \
             JOIN books b ON b.uuid = sb.book_uuid \
            WHERE sb.shelf_id IN ({placeholders}) AND {HAS_COVER} \
         ) WHERE rn <= {MOSAIC_COVERS} ORDER BY shelf_id, rn"
    );
    let mut q = sqlx::query(&sql);
    for id in shelf_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    for r in &rows {
        out.entry(r.try_get("shelf_id")?)
            .or_default()
            .push(r.try_get("book_uuid")?);
    }
    Ok(out)
}

/// First-[`MOSAIC_COVERS`] cover-bearing wishlist uuids per owner, newest
/// first (matching the detail path's wishlist fetch order), keyed by owner id
/// like [`count_wishlist_batch`].
async fn covers_wishlist_batch(
    pool: &SqlitePool,
    owner_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, ShelfError> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if owner_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = owner_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT uid, book_uuid FROM ( \
           SELECT we.user_id AS uid, we.book_uuid, \
                  ROW_NUMBER() OVER (PARTITION BY we.user_id \
                                     ORDER BY we.added_at DESC, we.id DESC) AS rn \
             FROM wishlist_entries we \
             JOIN books b ON b.uuid = we.book_uuid \
            WHERE we.user_id IN ({placeholders}) AND {HAS_COVER} \
         ) WHERE rn <= {MOSAIC_COVERS} ORDER BY uid, rn"
    );
    let mut q = sqlx::query(&sql);
    for id in owner_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await?;
    for r in &rows {
        out.entry(r.try_get("uid")?)
            .or_default()
            .push(r.try_get("book_uuid")?);
    }
    Ok(out)
}

/// First-[`MOSAIC_COVERS`] cover-bearing member uuids per smart shelf. Like
/// [`count_smart_fan_out`], each shelf's predicate is unique so these fan out
/// [`SMART_COUNT_CONCURRENCY`] at a time — but as uuid-only `LIMIT` queries,
/// not hydrated pages. Title order mirrors the landing default sort.
async fn covers_smart_fan_out(
    pool: &SqlitePool,
    inputs: &[(i64, i64, MatchMode, Vec<ShelfRule>)],
) -> Result<HashMap<i64, Vec<String>>, ShelfError> {
    async fn one(
        pool: &SqlitePool,
        owner_id: i64,
        mode: MatchMode,
        rules: &[ShelfRule],
    ) -> Result<Vec<String>, ShelfError> {
        let pred = membership_predicate(rules, mode, owner_id)?;
        let sql = format!(
            "SELECT b.uuid FROM books b \
             WHERE {SMART_VISIBLE} AND {HAS_COVER} AND {} \
             ORDER BY {} LIMIT {MOSAIC_COVERS}",
            pred.sql,
            order_by_sql(SortKey::Title, SortDir::Asc),
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

    let mut out = HashMap::with_capacity(inputs.len());
    for chunk in inputs.chunks(SMART_COUNT_CONCURRENCY) {
        let covers = try_join_all(
            chunk
                .iter()
                .map(|(_, owner_id, mode, rules)| one(pool, *owner_id, *mode, rules)),
        )
        .await?;
        for ((shelf_id, ..), uuids) in chunk.iter().zip(covers) {
            out.insert(*shelf_id, uuids);
        }
    }
    Ok(out)
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

/// Count the owner's wishlist. Membership is the user's `wishlist_entries`, not
/// `shelf_books` — the join to `books` is what keeps the count consistent with
/// the detail path's wishlist fetch.
///
/// Batched wishlist counts keyed by owner id, in one `GROUP BY we.user_id` pass
/// — the [`super::detail`] single-owner count's analogue for
/// [`list_visible_shelves`], so a rail showing many public wishlists isn't one
/// query per shelf. An owner with an empty wishlist has no row; callers
/// default a missing id to 0.
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
