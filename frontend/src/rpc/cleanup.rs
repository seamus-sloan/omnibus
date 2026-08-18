//! Library-cleanup review surface: the admin-gated server functions behind
//! the Settings section and the `/settings/cleanup/:kind` review page. Reads
//! and decides `dedup_suggestions` rows (migration `0069`) and hands an
//! accepted one to the `db::cleanup` apply primitives.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{CleanupCounts, CleanupKind, Decision, SuggestionCard};

// Only the server-side store decodes an action token; the client stubs never
// name one.
#[cfg(feature = "server")]
use omnibus_shared::CleanupAction;

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, PoolExt, WorkerExt};

/// How many pending cards one `cleanup/queue` call may return. The review
/// page works through one card at a time, so a page of this size is several
/// minutes of review while keeping the per-card hydration queries bounded.
pub const CLEANUP_QUEUE_MAX: i64 = 50;

// ---------------------------------------------------------------------------
// Server functions
// ---------------------------------------------------------------------------

/// Admin-only: pending/accepted/rejected counts for each cleanup kind, in a
/// stable order, for the Settings section's per-kind rows.
#[post("/api/rpc/cleanup/counts", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_counts() -> Result<Vec<(CleanupKind, CleanupCounts)>> {
    Ok(counts_by_kind(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("cleanup counts", e))?)
}

/// Admin-only: the pending review queue for one kind, oldest first, hydrated
/// into renderable cards. `limit` is clamped to [`CLEANUP_QUEUE_MAX`].
#[post("/api/rpc/cleanup/queue", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_queue(kind: CleanupKind, limit: i64) -> Result<Vec<SuggestionCard>> {
    Ok(
        pending_queue(&pool.0, kind, limit.clamp(1, CLEANUP_QUEUE_MAX))
            .await
            .map_err(|e| internal_rpc_error("cleanup queue", e))?,
    )
}

/// Admin-only: record a review decision on one suggestion. `Accepted` also
/// runs the matching apply primitive, so a card the admin accepted is applied
/// before this returns; `Rejected` only records the decision (the row stays as
/// the record that stops detection re-suggesting it). `Pending` is rejected —
/// a decision endpoint cannot un-decide.
#[post("/api/rpc/cleanup/decide", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_cleanup_decide(id: i64, decision: Decision) -> Result<()> {
    if decision == Decision::Pending {
        return Err(ServerFnError::new("decision must be accepted or rejected").into());
    }
    let row = load_suggestion(&pool.0, id)
        .await
        .map_err(|e| internal_rpc_error("load suggestion", e))?;
    let Some(row) = row else {
        return Err(ServerFnError::new("suggestion not found").into());
    };
    if row.decision != Decision::Pending {
        return Err(ServerFnError::new("suggestion has already been reviewed").into());
    }
    if decision == Decision::Accepted {
        apply_suggestion(&pool.0, &row, admin.0.id).await?;
    }
    record_decision(&pool.0, id, decision, admin.0.id)
        .await
        .map_err(|e| internal_rpc_error("record cleanup decision", e))?;
    Ok(())
}

/// Admin-only: queue a detection pass. `kind = None` runs every detector.
/// Returns the worker task id so the caller can poll the shared task status.
#[post("/api/rpc/cleanup/detect", _pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_cleanup_detect(kind: Option<CleanupKind>) -> Result<u64> {
    Ok(worker.0.post(db::worker::Task::DetectCleanup { kind }))
}

/// Admin-only: reverse an applied cleanup by its `cleanup_log` id.
#[post("/api/rpc/cleanup/undo", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_undo(log_id: i64) -> Result<()> {
    db::cleanup::undo(&pool.0, log_id)
        .await
        .map_err(|e| map_apply_error("cleanup undo", e))?;
    Ok(())
}

/// Admin-only: delete a taxonomy entity outright, outside the suggestion
/// queue — the on-page "this author is junk" escape hatch. Returns the
/// `cleanup_log` id so the caller can offer an undo. Only authors are
/// deletable; series/tag/book-title have no delete primitive.
#[post("/api/rpc/cleanup/delete-entity", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_cleanup_delete_entity(kind: CleanupKind, entity_id: i64) -> Result<i64> {
    if kind != CleanupKind::Author {
        return Err(ServerFnError::new("only authors can be deleted").into());
    }
    Ok(
        db::cleanup::apply_delete_author(&pool.0, entity_id, None, Some(admin.0.id))
            .await
            .map_err(|e| map_apply_error("cleanup delete entity", e))?,
    )
}

// ---------------------------------------------------------------------------
// Server-side suggestion store
//
// `db::cleanup` owns detection and apply/undo; the `dedup_suggestions` review
// queue itself has no db-layer API yet, so the read/decide queries live here
// with the routes that serve them.
// ---------------------------------------------------------------------------

/// Errors from the suggestion-store queries this module owns. Every variant
/// is an internal fault (a DB failure, or a stored row this build can't
/// decode) — none is user-actionable, so the routes genericize all of them.
#[cfg(feature = "server")]
#[derive(Debug, thiserror::Error)]
enum CleanupStoreError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("malformed cleanup suggestion payload: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("unrecognized cleanup token: {0}")]
    UnknownToken(String),
}

/// Map a `CleanupApplyError` to a client-facing error: the typed variants
/// already carry a safe, specific sentence; only the opaque ones are
/// genericized and logged.
#[cfg(feature = "server")]
fn map_apply_error(context: &'static str, e: db::CleanupApplyError) -> ServerFnError {
    match e {
        db::CleanupApplyError::Db(inner) => internal_rpc_error(context, inner),
        db::CleanupApplyError::Snapshot(inner) => internal_rpc_error(context, inner),
        other => ServerFnError::new(other.to_string()),
    }
}

/// One `dedup_suggestions` row, decoded.
#[cfg(feature = "server")]
#[derive(Debug)]
struct SuggestionRow {
    id: i64,
    kind: CleanupKind,
    action: CleanupAction,
    decision: Decision,
    payload: StoredPayload,
    created_at: i64,
}

/// The persisted `payload_json` shape. Mirrors `db::CleanupPayload`, which is
/// `Serialize`-only — this is the read side of the same wire format, minus the
/// fields the review surface never renders.
#[cfg(feature = "server")]
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StoredPayload {
    Merge {
        source_ids: Vec<i64>,
        source_names: Vec<String>,
        canonical_id: i64,
        canonical_name: String,
    },
    Split {
        source_id: i64,
        source_name: String,
        atoms: Vec<String>,
        delimiter: String,
    },
    Rename {
        book_uuid: String,
        current_title: String,
        proposed_title: String,
    },
    Delete {
        entity_id: i64,
        name: String,
    },
}

/// Per-kind review-state counts, in [`CleanupKind`]'s own declaration order so
/// the Settings rows never reshuffle between refreshes.
#[cfg(feature = "server")]
async fn counts_by_kind(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<(CleanupKind, CleanupCounts)>, sqlx::Error> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, decision, COUNT(*) FROM dedup_suggestions GROUP BY kind, decision",
    )
    .fetch_all(pool)
    .await?;

    let kinds = [
        CleanupKind::Author,
        CleanupKind::Series,
        CleanupKind::Tag,
        CleanupKind::BookTitle,
    ];
    let mut out: Vec<(CleanupKind, CleanupCounts)> = kinds
        .iter()
        .map(|k| (*k, CleanupCounts::default()))
        .collect();
    for (kind, decision, count) in rows {
        let (Some(kind), Some(decision)) =
            (CleanupKind::from_str(&kind), Decision::from_str(&decision))
        else {
            // A row written by a newer schema than this build understands is
            // not a reason to fail the whole dashboard.
            continue;
        };
        let Some(slot) = out.iter_mut().find(|(k, _)| *k == kind) else {
            continue;
        };
        match decision {
            Decision::Pending => slot.1.pending = count,
            Decision::Accepted => slot.1.accepted = count,
            Decision::Rejected => slot.1.rejected = count,
        }
    }
    Ok(out)
}

/// Load one suggestion by id, or `None` when it doesn't exist.
#[cfg(feature = "server")]
async fn load_suggestion(
    pool: &sqlx::SqlitePool,
    id: i64,
) -> Result<Option<SuggestionRow>, CleanupStoreError> {
    let row: Option<(i64, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, kind, action, decision, payload_json, created_at
           FROM dedup_suggestions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(decode_row).transpose()
}

/// Decode a raw `dedup_suggestions` tuple into a [`SuggestionRow`].
#[cfg(feature = "server")]
fn decode_row(
    (id, kind, action, decision, payload_json, created_at): (
        i64,
        String,
        String,
        String,
        String,
        i64,
    ),
) -> Result<SuggestionRow, CleanupStoreError> {
    let kind = CleanupKind::from_str(&kind).ok_or(CleanupStoreError::UnknownToken(kind.clone()))?;
    let action =
        CleanupAction::from_str(&action).ok_or(CleanupStoreError::UnknownToken(action.clone()))?;
    let decision =
        Decision::from_str(&decision).ok_or(CleanupStoreError::UnknownToken(decision.clone()))?;
    Ok(SuggestionRow {
        id,
        kind,
        action,
        decision,
        payload: serde_json::from_str(&payload_json)?,
        created_at,
    })
}

/// The oldest `limit` pending suggestions of one kind, hydrated into cards.
/// Hydration costs one small count query per card, which is why the queue is
/// paged rather than returning the whole backlog.
#[cfg(feature = "server")]
async fn pending_queue(
    pool: &sqlx::SqlitePool,
    kind: CleanupKind,
    limit: i64,
) -> Result<Vec<SuggestionCard>, CleanupStoreError> {
    let rows: Vec<(i64, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, kind, action, decision, payload_json, created_at
           FROM dedup_suggestions
          WHERE kind = ? AND decision = 'pending'
          ORDER BY id
          LIMIT ?",
    )
    .bind(kind.as_str())
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut cards = Vec::with_capacity(rows.len());
    for row in rows {
        let row = decode_row(row)?;
        cards.push(hydrate_card(pool, &row).await?);
    }
    Ok(cards)
}

/// Build the renderable card for one suggestion: the display names come from
/// the payload, the book count and photo from the live library.
#[cfg(feature = "server")]
async fn hydrate_card(
    pool: &sqlx::SqlitePool,
    row: &SuggestionRow,
) -> Result<SuggestionCard, CleanupStoreError> {
    let (primary_name, secondary_name) = card_names(&row.payload);
    let book_count = affected_book_count(pool, row).await?;
    let photo_url = card_photo_url(pool, row).await?;
    Ok(SuggestionCard {
        id: row.id,
        kind: row.kind,
        action: row.action,
        decision: row.decision,
        primary_name,
        secondary_name,
        book_count,
        photo_url,
        created_at: row.created_at,
    })
}

/// The card's primary/secondary display names. A merge names its survivor and
/// — only when exactly one entity is being merged away — the name that goes;
/// a rename names the current title and the proposed one.
#[cfg(feature = "server")]
fn card_names(payload: &StoredPayload) -> (String, Option<String>) {
    match payload {
        StoredPayload::Merge {
            source_names,
            canonical_name,
            ..
        } => {
            let secondary = match source_names.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            };
            (canonical_name.clone(), secondary)
        }
        StoredPayload::Split { source_name, .. } => (source_name.clone(), None),
        StoredPayload::Rename {
            current_title,
            proposed_title,
            ..
        } => (current_title.clone(), Some(proposed_title.clone())),
        StoredPayload::Delete { name, .. } => (name.clone(), None),
    }
}

/// How many distinct books the suggestion touches if applied.
#[cfg(feature = "server")]
async fn affected_book_count(
    pool: &sqlx::SqlitePool,
    row: &SuggestionRow,
) -> Result<i64, sqlx::Error> {
    let (table, column) = match row.kind {
        CleanupKind::Author => ("books_authors_link", "author"),
        CleanupKind::Series => ("books_series_link", "series"),
        CleanupKind::Tag => ("books_tags_link", "tag"),
        // A title rename is scoped to its one book by construction.
        CleanupKind::BookTitle => return Ok(1),
    };
    let ids: Vec<i64> = match &row.payload {
        StoredPayload::Merge {
            source_ids,
            canonical_id,
            ..
        } => source_ids.iter().copied().chain([*canonical_id]).collect(),
        StoredPayload::Split { source_id, .. } => vec![*source_id],
        StoredPayload::Delete { entity_id, .. } => vec![*entity_id],
        StoredPayload::Rename { .. } => return Ok(1),
    };
    count_linked_books(pool, table, column, &ids).await
}

/// `COUNT(DISTINCT book)` over a taxonomy link table for a set of entity ids.
/// The placeholder list is generated from the id count, never interpolated
/// from caller text, so this stays a bound-parameter query.
#[cfg(feature = "server")]
async fn count_linked_books(
    pool: &sqlx::SqlitePool,
    table: &str,
    column: &str,
    ids: &[i64],
) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql =
        format!("SELECT COUNT(DISTINCT book) FROM {table} WHERE {column} IN ({placeholders})");
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids {
        query = query.bind(*id);
    }
    query.fetch_one(pool).await
}

/// The card's photo, when the kind has one. Only authors carry a cached image
/// (`author_photos`); series and tags have no photo endpoint, and a `letter`
/// placeholder row carries no bytes so it isn't worth a round trip.
#[cfg(feature = "server")]
async fn card_photo_url(
    pool: &sqlx::SqlitePool,
    row: &SuggestionRow,
) -> Result<Option<String>, sqlx::Error> {
    if row.kind != CleanupKind::Author {
        return Ok(None);
    }
    let author_id = match &row.payload {
        StoredPayload::Merge { canonical_id, .. } => *canonical_id,
        StoredPayload::Delete { entity_id, .. } => *entity_id,
        _ => return Ok(None),
    };
    let has_photo: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM author_photos WHERE author_id = ? AND bytes IS NOT NULL")
            .bind(author_id)
            .fetch_optional(pool)
            .await?;
    Ok(has_photo.map(|_| format!("/api/authors/{author_id}/photo")))
}

/// Run the apply primitive an accepted suggestion names. Every unsupported
/// `(kind, action)` pair is a stored row this build has no primitive for, so
/// it reports rather than silently recording an accept that did nothing.
#[cfg(feature = "server")]
async fn apply_suggestion(
    pool: &sqlx::SqlitePool,
    row: &SuggestionRow,
    admin_id: i64,
) -> Result<(), ServerFnError> {
    let id = Some(row.id);
    let by = Some(admin_id);
    let applied = match (row.kind, row.action, &row.payload) {
        (
            CleanupKind::Author,
            CleanupAction::Merge,
            StoredPayload::Merge {
                source_ids,
                canonical_id,
                ..
            },
        ) => db::cleanup::apply_merge_authors(pool, source_ids, *canonical_id, id, by).await,
        (
            CleanupKind::Series,
            CleanupAction::Merge,
            StoredPayload::Merge {
                source_ids,
                canonical_id,
                ..
            },
        ) => db::cleanup::apply_merge_series(pool, source_ids, *canonical_id, id, by).await,
        (
            CleanupKind::Tag,
            CleanupAction::Merge,
            StoredPayload::Merge {
                source_ids,
                canonical_id,
                ..
            },
        ) => db::cleanup::apply_merge_tags(pool, source_ids, *canonical_id, id, by).await,
        (
            CleanupKind::Tag,
            CleanupAction::Split,
            StoredPayload::Split {
                source_id,
                atoms,
                delimiter,
                ..
            },
        ) => db::cleanup::apply_tag_split(pool, *source_id, delimiter, atoms, id, by).await,
        (
            CleanupKind::BookTitle,
            CleanupAction::Rename,
            StoredPayload::Rename {
                book_uuid,
                proposed_title,
                ..
            },
        ) => {
            db::cleanup::apply_book_title_override(pool, book_uuid, proposed_title, id, admin_id)
                .await
        }
        (CleanupKind::Author, CleanupAction::Delete, StoredPayload::Delete { entity_id, .. }) => {
            db::cleanup::apply_delete_author(pool, *entity_id, id, by).await
        }
        _ => {
            return Err(ServerFnError::new(
                "this suggestion cannot be applied automatically",
            ))
        }
    };
    applied.map_err(|e| map_apply_error("apply cleanup suggestion", e))?;
    Ok(())
}

/// Stamp the decision, the reviewing admin, and the review time onto the row.
#[cfg(feature = "server")]
async fn record_decision(
    pool: &sqlx::SqlitePool,
    id: i64,
    decision: Decision,
    admin_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dedup_suggestions
            SET decision = ?, decided_at = strftime('%s', 'now'), decided_by = ?
          WHERE id = ?",
    )
    .bind(decision.as_str())
    .bind(admin_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(all(test, feature = "server"))]
mod tests;
