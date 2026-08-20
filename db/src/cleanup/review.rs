//! The `dedup_suggestions` review queue (migration `0069`): per-kind counts,
//! the pending queue hydrated into renderable cards, and the decide path that
//! runs the matching [`super::apply`] primitive before stamping a decision.
//! Detection writes these rows; this module reads and retires them.

use omnibus_shared::{CleanupAction, CleanupCounts, CleanupKind, Decision, SuggestionCard};
use sqlx::SqlitePool;

use crate::helpers::ids_json;

use super::{
    apply_book_title_override, apply_delete_author, apply_merge_authors, apply_merge_series,
    apply_merge_tags, apply_tag_split, CleanupApplyError, CleanupPayload,
};

#[cfg(test)]
mod tests;

/// How many pending cards one [`review_queue`] call may return. The review
/// surface works through one card at a time, so a page of this size is several
/// minutes of review while keeping the per-card hydration queries bounded.
pub const REVIEW_QUEUE_MAX: i64 = 50;

/// Errors from the review queue. The first three are internal faults (a DB
/// failure, or a stored row this build can't decode); the rest are decisions
/// a caller can render verbatim.
#[derive(Debug, thiserror::Error)]
pub enum CleanupStoreError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("malformed cleanup suggestion payload: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("unrecognized cleanup token: {0}")]
    UnknownToken(String),
    #[error("decision must be accepted or rejected")]
    NotADecision,
    #[error("suggestion not found")]
    NotFound,
    #[error("suggestion has already been reviewed")]
    AlreadyReviewed,
    #[error("this suggestion cannot be applied automatically")]
    Unsupported,
    #[error(transparent)]
    Apply(#[from] CleanupApplyError),
}

/// One `dedup_suggestions` row, decoded.
#[derive(Debug)]
struct SuggestionRow {
    id: i64,
    kind: CleanupKind,
    action: CleanupAction,
    decision: Decision,
    payload: CleanupPayload,
    created_at: i64,
}

/// The column tuple every `dedup_suggestions` read selects.
type RawRow = (i64, String, String, String, String, i64);

/// Per-kind review-state counts, in [`CleanupKind`]'s own declaration order so
/// a caller's rows never reshuffle between refreshes.
pub async fn review_counts(
    pool: &SqlitePool,
) -> Result<Vec<(CleanupKind, CleanupCounts)>, CleanupStoreError> {
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

/// The oldest pending suggestions of one kind, hydrated into cards. `limit` is
/// clamped to `[1, REVIEW_QUEUE_MAX]` — hydration costs one small count query
/// per card, which is why the queue is paged rather than returning the whole
/// backlog.
pub async fn review_queue(
    pool: &SqlitePool,
    kind: CleanupKind,
    limit: i64,
) -> Result<Vec<SuggestionCard>, CleanupStoreError> {
    let rows: Vec<RawRow> = sqlx::query_as(
        "SELECT id, kind, action, decision, payload_json, created_at
           FROM dedup_suggestions
          WHERE kind = ? AND decision = 'pending'
          ORDER BY id
          LIMIT ?",
    )
    .bind(kind.as_str())
    .bind(limit.clamp(1, REVIEW_QUEUE_MAX))
    .fetch_all(pool)
    .await?;

    let mut cards = Vec::with_capacity(rows.len());
    for row in rows {
        let row = decode_row(row)?;
        cards.push(hydrate_card(pool, &row).await?);
    }
    Ok(cards)
}

/// Record a review decision on one pending suggestion.
///
/// [`Decision::Accepted`] runs the matching apply primitive *before* the
/// decision is stamped, so an accept that could not be applied never records
/// as applied. [`Decision::Rejected`] only stamps (the row stays as the record
/// that stops detection re-suggesting it). [`Decision::Pending`] is refused —
/// this path cannot un-decide a row.
pub async fn decide_suggestion(
    pool: &SqlitePool,
    id: i64,
    decision: Decision,
    admin_id: i64,
) -> Result<(), CleanupStoreError> {
    if decision == Decision::Pending {
        return Err(CleanupStoreError::NotADecision);
    }
    let row = load_suggestion(pool, id)
        .await?
        .ok_or(CleanupStoreError::NotFound)?;
    if row.decision != Decision::Pending {
        return Err(CleanupStoreError::AlreadyReviewed);
    }
    if decision == Decision::Accepted {
        apply_suggestion(pool, &row, admin_id).await?;
    }
    record_decision(pool, id, decision, admin_id).await?;
    Ok(())
}

/// Load one suggestion by id, or `None` when it doesn't exist.
async fn load_suggestion(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<SuggestionRow>, CleanupStoreError> {
    let row: Option<RawRow> = sqlx::query_as(
        "SELECT id, kind, action, decision, payload_json, created_at
           FROM dedup_suggestions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(decode_row).transpose()
}

/// Decode a raw `dedup_suggestions` tuple into a [`SuggestionRow`].
fn decode_row(
    (id, kind, action, decision, payload_json, created_at): RawRow,
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

/// Build the renderable card for one suggestion: the display names come from
/// the payload, the book count and photo from the live library.
async fn hydrate_card(
    pool: &SqlitePool,
    row: &SuggestionRow,
) -> Result<SuggestionCard, CleanupStoreError> {
    let (primary_name, secondary_name) = card_names(&row.payload);
    Ok(SuggestionCard {
        id: row.id,
        kind: row.kind,
        action: row.action,
        decision: row.decision,
        primary_name,
        secondary_name,
        book_count: affected_book_count(pool, row).await?,
        photo_url: card_photo_url(pool, row).await?,
        created_at: row.created_at,
    })
}

/// The card's primary/secondary display names. A merge names its survivor and
/// — only when exactly one entity is being merged away — the name that goes;
/// a rename names the current title and the proposed one.
fn card_names(payload: &CleanupPayload) -> (String, Option<String>) {
    match payload {
        CleanupPayload::Merge {
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
        CleanupPayload::Split { source_name, .. } => (source_name.clone(), None),
        CleanupPayload::Rename {
            current_title,
            proposed_title,
            ..
        } => (current_title.clone(), Some(proposed_title.clone())),
        CleanupPayload::Delete { name, .. } => (name.clone(), None),
    }
}

/// How many distinct books the suggestion touches if applied.
async fn affected_book_count(pool: &SqlitePool, row: &SuggestionRow) -> Result<i64, sqlx::Error> {
    let (table, column) = match row.kind {
        CleanupKind::Author => ("books_authors_link", "author"),
        CleanupKind::Series => ("books_series_link", "series"),
        CleanupKind::Tag => ("books_tags_link", "tag"),
        // A title rename is scoped to its one book by construction.
        CleanupKind::BookTitle => return Ok(1),
    };
    let ids: Vec<i64> = match &row.payload {
        CleanupPayload::Merge {
            source_ids,
            canonical_id,
            ..
        } => source_ids.iter().copied().chain([*canonical_id]).collect(),
        CleanupPayload::Split { source_id, .. } => vec![*source_id],
        CleanupPayload::Delete { entity_id, .. } => vec![*entity_id],
        CleanupPayload::Rename { .. } => return Ok(1),
    };
    count_linked_books(pool, table, column, &ids).await
}

/// `COUNT(DISTINCT book)` over a taxonomy link table for a set of entity ids.
///
/// The ids arrive as a single JSON array bound to one placeholder rather than
/// a generated `IN (?, ?, …)` list: nothing caps a merge group's size (see
/// `merge_suggestion`), so a large duplicate cluster would otherwise blow
/// `SQLITE_MAX_VARIABLE_NUMBER` and fail the *whole* queue hydration, not just
/// that one card. `table` and `column` are module constants chosen by
/// [`CleanupKind`], never caller text.
async fn count_linked_books(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    ids: &[i64],
) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "SELECT COUNT(DISTINCT book) FROM {table} \
          WHERE {column} IN (SELECT value FROM json_each(?))"
    );
    sqlx::query_scalar(&sql)
        .bind(ids_json(ids))
        .fetch_one(pool)
        .await
}

/// The card's photo, when the kind has one. Only authors carry a cached image
/// (`author_photos`); series and tags have no photo endpoint, and a `letter`
/// placeholder row carries no bytes so it isn't worth a round trip.
async fn card_photo_url(
    pool: &SqlitePool,
    row: &SuggestionRow,
) -> Result<Option<String>, sqlx::Error> {
    if row.kind != CleanupKind::Author {
        return Ok(None);
    }
    let author_id = match &row.payload {
        CleanupPayload::Merge { canonical_id, .. } => *canonical_id,
        CleanupPayload::Delete { entity_id, .. } => *entity_id,
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
async fn apply_suggestion(
    pool: &SqlitePool,
    row: &SuggestionRow,
    admin_id: i64,
) -> Result<(), CleanupStoreError> {
    let id = Some(row.id);
    let by = Some(admin_id);
    match (row.kind, row.action, &row.payload) {
        (
            CleanupKind::Author,
            CleanupAction::Merge,
            CleanupPayload::Merge {
                source_ids,
                canonical_id,
                ..
            },
        ) => apply_merge_authors(pool, source_ids, *canonical_id, id, by).await?,
        (
            CleanupKind::Series,
            CleanupAction::Merge,
            CleanupPayload::Merge {
                source_ids,
                canonical_id,
                ..
            },
        ) => apply_merge_series(pool, source_ids, *canonical_id, id, by).await?,
        (
            CleanupKind::Tag,
            CleanupAction::Merge,
            CleanupPayload::Merge {
                source_ids,
                canonical_id,
                ..
            },
        ) => apply_merge_tags(pool, source_ids, *canonical_id, id, by).await?,
        (
            CleanupKind::Tag,
            CleanupAction::Split,
            CleanupPayload::Split {
                source_id,
                atoms,
                delimiter,
                ..
            },
        ) => apply_tag_split(pool, *source_id, delimiter, atoms, id, by).await?,
        (
            CleanupKind::BookTitle,
            CleanupAction::Rename,
            CleanupPayload::Rename {
                book_uuid,
                proposed_title,
                ..
            },
        ) => apply_book_title_override(pool, book_uuid, proposed_title, id, admin_id).await?,
        (CleanupKind::Author, CleanupAction::Delete, CleanupPayload::Delete { entity_id, .. }) => {
            apply_delete_author(pool, *entity_id, id, by).await?
        }
        _ => return Err(CleanupStoreError::Unsupported),
    };
    Ok(())
}

/// Stamp the decision, the reviewing admin, and the review time onto the row.
async fn record_decision(
    pool: &SqlitePool,
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
