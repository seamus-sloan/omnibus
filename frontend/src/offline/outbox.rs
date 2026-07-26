//! Durable mutation outbox: every user write that fails with a
//! connectivity-class error (or is attempted while already offline) is
//! queued here, applied optimistically to the replica cache, and replayed
//! in enqueue order by the sync drain. Conflict stance is last-write-wins
//! offline-created rows carry
//! negative client temp ids that remap to server ids on drain.

use omnibus_shared::{
    AudiobookPlaybackRateRecord, AudiobookPlaybackRateUpdate, Bookmark, CreateBookmark,
    CreateHighlight, CreateJournalEntry, CreateShelfRequest, Highlight, HighlightColor,
    JournalEntry, ProgressRecord, ProgressUpdate, RatingRecord, RatingUpdate, SessionReport, Shelf,
    UpdateJournalEntry, UpdateShelfRequest, UserSummary,
};

use super::{cache, store, sync};

pub(crate) mod apply;

/// One queued mutation, serialized as tagged JSON in the `ops` table.
/// `book_uuid` fields on by-id edits exist purely so the optimistic cache
/// patch knows which per-book list to touch; empty when unresolvable.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Op {
    SaveProgress {
        update: ProgressUpdate,
        /// Client wall-clock at capture — the drain's stale guard compares
        /// it against the server record's `updated_at`.
        captured_at: i64,
    },
    SetPlaybackRate {
        uuid: String,
        update: AudiobookPlaybackRateUpdate,
    },
    RecordSessions {
        reports: Vec<SessionReport>,
    },
    SetRating {
        update: RatingUpdate,
    },
    ClearRating {
        uuid: String,
    },
    CreateHighlight {
        temp_id: i64,
        input: CreateHighlight,
    },
    UpdateHighlightColor {
        id: i64,
        book_uuid: String,
        color: HighlightColor,
    },
    UpdateHighlightNote {
        id: i64,
        book_uuid: String,
        note: Option<String>,
    },
    DeleteHighlight {
        id: i64,
        book_uuid: String,
    },
    CreateBookmark {
        temp_id: i64,
        input: CreateBookmark,
    },
    UpdateBookmark {
        id: i64,
        book_uuid: String,
        title: Option<String>,
    },
    DeleteBookmark {
        id: i64,
        book_uuid: String,
    },
    CreateShelf {
        temp_id: i64,
        req: CreateShelfRequest,
    },
    UpdateShelf {
        id: i64,
        req: UpdateShelfRequest,
    },
    DeleteShelf {
        id: i64,
    },
    AddShelfBooks {
        shelf_id: i64,
        book_uuids: Vec<String>,
    },
    RemoveShelfBook {
        shelf_id: i64,
        book_uuid: String,
    },
    CreateJournal {
        temp_id: i64,
        input: CreateJournalEntry,
    },
    UpdateJournal {
        id: i64,
        book_uuid: String,
        input: UpdateJournalEntry,
    },
    DeleteJournal {
        id: i64,
        book_uuid: String,
    },
    SetKindleEmail {
        email: Option<String>,
    },
}

impl Op {
    /// Tag stored in the `ops.kind` column (debuggability; dispatch uses the
    /// serde payload).
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Op::SaveProgress { .. } => "SaveProgress",
            Op::SetPlaybackRate { .. } => "SetPlaybackRate",
            Op::RecordSessions { .. } => "RecordSessions",
            Op::SetRating { .. } => "SetRating",
            Op::ClearRating { .. } => "ClearRating",
            Op::CreateHighlight { .. } => "CreateHighlight",
            Op::UpdateHighlightColor { .. } => "UpdateHighlightColor",
            Op::UpdateHighlightNote { .. } => "UpdateHighlightNote",
            Op::DeleteHighlight { .. } => "DeleteHighlight",
            Op::CreateBookmark { .. } => "CreateBookmark",
            Op::UpdateBookmark { .. } => "UpdateBookmark",
            Op::DeleteBookmark { .. } => "DeleteBookmark",
            Op::CreateShelf { .. } => "CreateShelf",
            Op::UpdateShelf { .. } => "UpdateShelf",
            Op::DeleteShelf { .. } => "DeleteShelf",
            Op::AddShelfBooks { .. } => "AddShelfBooks",
            Op::RemoveShelfBook { .. } => "RemoveShelfBook",
            Op::CreateJournal { .. } => "CreateJournal",
            Op::UpdateJournal { .. } => "UpdateJournal",
            Op::DeleteJournal { .. } => "DeleteJournal",
            Op::SetKindleEmail { .. } => "SetKindleEmail",
        }
    }

    /// Local coalesce key: a queued op with the same key is replaced by a
    /// newer one (idempotent upserts and whole-value edits only — partial
    /// patches like `UpdateShelf` must retain every queued step).
    pub(crate) fn coalesce_key(&self) -> Option<String> {
        match self {
            Op::SaveProgress { update, .. } => Some(format!(
                "prog:{}:{}",
                update.book_uuid,
                format_key(update.format)
            )),
            Op::SetPlaybackRate { uuid, .. } => Some(format!("rate:{uuid}")),
            Op::SetRating { update } => Some(format!("rating:{}", update.book_uuid)),
            Op::ClearRating { uuid } => Some(format!("rating:{uuid}")),
            Op::UpdateHighlightColor { id, .. } => Some(format!("hl:{id}:color")),
            Op::UpdateHighlightNote { id, .. } => Some(format!("hl:{id}:note")),
            Op::UpdateBookmark { id, .. } => Some(format!("bm:{id}:title")),
            Op::UpdateJournal { id, .. } => Some(format!("journal:{id}")),
            Op::SetKindleEmail { .. } => Some("kindle_email".into()),
            _ => None,
        }
    }

    /// The negative temp id this op *creates*, if any.
    fn created_temp_id(&self) -> Option<i64> {
        match self {
            Op::CreateHighlight { temp_id, .. }
            | Op::CreateBookmark { temp_id, .. }
            | Op::CreateShelf { temp_id, .. }
            | Op::CreateJournal { temp_id, .. } => Some(*temp_id),
            _ => None,
        }
    }

    /// Every entity id this op references (for temp-cancel and remap). The
    /// global temp-id counter means a given negative id belongs to exactly
    /// one entity, so a generic match is safe across families.
    fn referenced_id(&self) -> Option<i64> {
        match self {
            Op::UpdateHighlightColor { id, .. }
            | Op::UpdateHighlightNote { id, .. }
            | Op::DeleteHighlight { id, .. }
            | Op::UpdateBookmark { id, .. }
            | Op::DeleteBookmark { id, .. }
            | Op::UpdateShelf { id, .. }
            | Op::DeleteShelf { id }
            | Op::UpdateJournal { id, .. }
            | Op::DeleteJournal { id, .. } => Some(*id),
            Op::AddShelfBooks { shelf_id, .. } | Op::RemoveShelfBook { shelf_id, .. } => {
                Some(*shelf_id)
            }
            _ => None,
        }
    }

    /// Rewrite `old_id` → `new_id` in this op's referenced-id field.
    /// Returns `true` when anything changed.
    pub(crate) fn remap_id(&mut self, old_id: i64, new_id: i64) -> bool {
        let slot = match self {
            Op::UpdateHighlightColor { id, .. }
            | Op::UpdateHighlightNote { id, .. }
            | Op::DeleteHighlight { id, .. }
            | Op::UpdateBookmark { id, .. }
            | Op::DeleteBookmark { id, .. }
            | Op::UpdateShelf { id, .. }
            | Op::DeleteShelf { id }
            | Op::UpdateJournal { id, .. }
            | Op::DeleteJournal { id, .. } => id,
            Op::AddShelfBooks { shelf_id, .. } | Op::RemoveShelfBook { shelf_id, .. } => shelf_id,
            _ => return false,
        };
        if *slot == old_id {
            *slot = new_id;
            true
        } else {
            false
        }
    }
}

/// Wire token for a progress format (matches the REST `?format=` values).
pub(crate) fn format_key(format: omnibus_shared::ProgressFormat) -> &'static str {
    match format {
        omnibus_shared::ProgressFormat::Epub => "epub",
        omnibus_shared::ProgressFormat::Audio => "audio",
    }
}

/// Append `op` to the durable queue (coalescing where safe) and refresh the
/// pending-count channel. `false` when the store is unavailable — callers
/// then surface the original network error instead of pretending success.
pub(crate) async fn enqueue(op: Op) -> bool {
    let Some(st) = store::store() else {
        return false;
    };
    let Ok(payload) = serde_json::to_string(&op) else {
        return false;
    };
    st.ops_append(op.kind(), payload, op.coalesce_key());
    sync::refresh_pending().await;
    true
}

/// Allocate the next negative temp id, or `None` without a store.
async fn temp_id() -> Option<i64> {
    store::store()?.next_temp_id().await
}

/// Cancel every queued op that created or references the temp id (deleting
/// an offline-created entity needs no server round trip at all).
pub(crate) async fn cancel_temp(id: i64) {
    let Some(st) = store::store() else { return };
    let mut doomed = Vec::new();
    for row in st.ops_list().await {
        let Ok(op) = serde_json::from_str::<Op>(&row.payload) else {
            continue;
        };
        if op.created_temp_id() == Some(id) || op.referenced_id() == Some(id) {
            doomed.push(row.id);
        }
    }
    st.ops_delete_many(doomed).await;
    sync::refresh_pending().await;
}

/// Rewrite `temp_id` → `real_id` in every queued op (drain remap after a
/// successful create).
pub(crate) async fn remap_queued(temp_id: i64, real_id: i64) {
    let Some(st) = store::store() else { return };
    st.ops_rewrite(move |_kind, payload| {
        let mut op = serde_json::from_str::<Op>(payload).ok()?;
        op.remap_id(temp_id, real_id)
            .then(|| serde_json::to_string(&op).ok())
            .flatten()
    })
    .await;
}

// ── queue_* entry points (called from the data-layer write wrappers) ──
//
// Each applies the mutation optimistically to the replica cache, enqueues
// the op, and synthesizes the value the online call would have returned.
// All return `None`/`false` when the offline store is unavailable.

/// Queue a reading/listening position write.
pub(crate) async fn queue_save_progress(update: &ProgressUpdate) -> Option<ProgressRecord> {
    let now = store::now_secs();
    let record = ProgressRecord {
        book_uuid: update.book_uuid.clone(),
        format: update.format,
        epub_cfi: update.epub_cfi.clone(),
        audio_position_seconds: update.audio_position_seconds,
        updated_at: now,
        // Optimistic local record: mirrors the server's own COALESCE (issue
        // #1362) — the update's own client event time when it sent one,
        // else "now".
        client_updated_at: update.client_updated_at.unwrap_or(now),
    };
    let queued = enqueue(Op::SaveProgress {
        update: update.clone(),
        captured_at: record.updated_at,
    })
    .await;
    if !queued {
        return None;
    }
    apply::progress_saved(&record).await;
    Some(record)
}

/// Queue a playback-rate change.
pub(crate) async fn queue_set_playback_rate(
    uuid: &str,
    update: &AudiobookPlaybackRateUpdate,
) -> Option<AudiobookPlaybackRateRecord> {
    let record = AudiobookPlaybackRateRecord {
        book_uuid: uuid.to_string(),
        playback_rate: update.playback_rate,
        updated_at: store::now_secs(),
    };
    let queued = enqueue(Op::SetPlaybackRate {
        uuid: uuid.to_string(),
        update: update.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    cache::put_json(&cache::keys::playback_rate(uuid), &Some(record.clone()));
    Some(record)
}

/// Queue a session-stats batch.
pub(crate) async fn queue_record_sessions(reports: &[SessionReport]) -> Option<u64> {
    let count = reports.len() as u64;
    enqueue(Op::RecordSessions {
        reports: reports.to_vec(),
    })
    .await
    .then_some(count)
}

/// Queue a rating upsert.
pub(crate) async fn queue_set_rating(update: &RatingUpdate) -> Option<RatingRecord> {
    let record = RatingRecord {
        book_uuid: update.book_uuid.clone(),
        stars: update.stars,
        updated_at: store::now_secs(),
    };
    let queued = enqueue(Op::SetRating {
        update: update.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    cache::put_json(
        &cache::keys::rating(&update.book_uuid),
        &Some(record.clone()),
    );
    Some(record)
}

/// Queue a rating clear.
pub(crate) async fn queue_clear_rating(uuid: &str) -> bool {
    let queued = enqueue(Op::ClearRating {
        uuid: uuid.to_string(),
    })
    .await;
    if queued {
        cache::put_json(&cache::keys::rating(uuid), &None::<RatingRecord>);
    }
    queued
}

/// Queue a highlight creation; the synthesized record carries a temp id.
pub(crate) async fn queue_create_highlight(input: &CreateHighlight) -> Option<Highlight> {
    let temp = temp_id().await?;
    let highlight = Highlight {
        id: temp,
        book_uuid: input.book_uuid.clone(),
        epub_cfi_range: input.epub_cfi_range.clone(),
        color: input.color,
        note: None,
        text: input.text.clone(),
        // Web addresses this row by its temp id and rewrites it on apply;
        // `client_id` is the mobile outbox's handle, not this one's.
        client_id: None,
        created_at: store::now_secs(),
    };
    let queued = enqueue(Op::CreateHighlight {
        temp_id: temp,
        input: input.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    apply::highlight_created(&highlight).await;
    Some(highlight)
}

/// Queue a highlight color change.
pub(crate) async fn queue_update_highlight_color(id: i64, color: HighlightColor) -> bool {
    let book_uuid = apply::find_highlight_book(id).await.unwrap_or_default();
    let queued = enqueue(Op::UpdateHighlightColor {
        id,
        book_uuid: book_uuid.clone(),
        color,
    })
    .await;
    if queued {
        apply::highlight_color_changed(&book_uuid, id, color).await;
    }
    queued
}

/// Queue a highlight note edit.
pub(crate) async fn queue_update_highlight_note(id: i64, note: Option<String>) -> bool {
    let book_uuid = apply::find_highlight_book(id).await.unwrap_or_default();
    let queued = enqueue(Op::UpdateHighlightNote {
        id,
        book_uuid: book_uuid.clone(),
        note: note.clone(),
    })
    .await;
    if queued {
        apply::highlight_note_changed(&book_uuid, id, note).await;
    }
    queued
}

/// Queue a highlight deletion (an unsynced temp highlight just cancels).
pub(crate) async fn queue_delete_highlight(id: i64) -> bool {
    let book_uuid = apply::find_highlight_book(id).await.unwrap_or_default();
    if id < 0 {
        cancel_temp(id).await;
        apply::highlight_deleted(&book_uuid, id).await;
        return true;
    }
    let queued = enqueue(Op::DeleteHighlight {
        id,
        book_uuid: book_uuid.clone(),
    })
    .await;
    if queued {
        apply::highlight_deleted(&book_uuid, id).await;
    }
    queued
}

/// Queue a bookmark creation; the synthesized record carries a temp id.
pub(crate) async fn queue_create_bookmark(input: &CreateBookmark) -> Option<Bookmark> {
    let temp = temp_id().await?;
    let bookmark = Bookmark {
        id: temp,
        book_uuid: input.book_uuid.clone(),
        position: input.position.clone(),
        title: input.title.clone(),
        client_id: None,
        created_at: store::now_secs(),
    };
    let queued = enqueue(Op::CreateBookmark {
        temp_id: temp,
        input: input.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    apply::bookmark_created(&bookmark).await;
    Some(bookmark)
}

/// Queue a bookmark rename.
pub(crate) async fn queue_update_bookmark(id: i64, title: Option<String>) -> bool {
    let book_uuid = apply::find_bookmark_book(id).await.unwrap_or_default();
    let queued = enqueue(Op::UpdateBookmark {
        id,
        book_uuid: book_uuid.clone(),
        title: title.clone(),
    })
    .await;
    if queued {
        apply::bookmark_renamed(&book_uuid, id, title).await;
    }
    queued
}

/// Queue a bookmark deletion (an unsynced temp bookmark just cancels).
pub(crate) async fn queue_delete_bookmark(id: i64) -> bool {
    let book_uuid = apply::find_bookmark_book(id).await.unwrap_or_default();
    if id < 0 {
        cancel_temp(id).await;
        apply::bookmark_deleted(&book_uuid, id).await;
        return true;
    }
    let queued = enqueue(Op::DeleteBookmark {
        id,
        book_uuid: book_uuid.clone(),
    })
    .await;
    if queued {
        apply::bookmark_deleted(&book_uuid, id).await;
    }
    queued
}

/// Queue a shelf creation; the synthesized shelf carries a temp id and the
/// cached account identity for the owner fields.
pub(crate) async fn queue_create_shelf(req: &CreateShelfRequest) -> Option<Shelf> {
    let temp = temp_id().await?;
    let me: Option<UserSummary> = cache::get_json(&cache::keys::me()).await;
    let shelf = Shelf {
        id: temp,
        owner_user_id: me.as_ref().map(|u| u.id).unwrap_or_default(),
        owner_username: me.map(|u| u.username).unwrap_or_default(),
        kind: req.kind,
        name: req.name.clone(),
        description: req.description.clone(),
        visibility: req.visibility,
        accent: None,
        match_mode: req.match_mode,
        rules: req.rules.clone(),
        book_count: req.book_uuids.len() as i64,
        // A locally-created shelf is never Kobo-synced until the user opts it
        // in against the server (#924).
        sync_to_kobo: false,
    };
    let queued = enqueue(Op::CreateShelf {
        temp_id: temp,
        req: req.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    apply::shelf_created(&shelf).await;
    Some(shelf)
}

/// Queue a shelf edit; returns the patched shelf from cache (or a best-
/// effort reconstruction when the shelf was never cached).
pub(crate) async fn queue_update_shelf(id: i64, req: &UpdateShelfRequest) -> Option<Shelf> {
    let queued = enqueue(Op::UpdateShelf {
        id,
        req: req.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    apply::shelf_updated(id, req).await
}

/// Queue a shelf deletion (an unsynced temp shelf just cancels).
pub(crate) async fn queue_delete_shelf(id: i64) -> bool {
    if id < 0 {
        cancel_temp(id).await;
        apply::shelf_deleted(id).await;
        return true;
    }
    let queued = enqueue(Op::DeleteShelf { id }).await;
    if queued {
        apply::shelf_deleted(id).await;
    }
    queued
}

/// Queue adding books to a manual shelf.
pub(crate) async fn queue_add_shelf_books(shelf_id: i64, book_uuids: &[String]) -> bool {
    let queued = enqueue(Op::AddShelfBooks {
        shelf_id,
        book_uuids: book_uuids.to_vec(),
    })
    .await;
    if queued {
        apply::shelf_books_added(shelf_id, book_uuids).await;
    }
    queued
}

/// Queue removing a book from a manual shelf.
pub(crate) async fn queue_remove_shelf_book(shelf_id: i64, book_uuid: &str) -> bool {
    let queued = enqueue(Op::RemoveShelfBook {
        shelf_id,
        book_uuid: book_uuid.to_string(),
    })
    .await;
    if queued {
        apply::shelf_book_removed(shelf_id, book_uuid).await;
    }
    queued
}

/// Queue a journal-entry creation. `body_html` is approximated locally
/// (escaped paragraphs); the server's render supersedes it after drain.
pub(crate) async fn queue_create_journal(input: &CreateJournalEntry) -> Option<JournalEntry> {
    let temp = temp_id().await?;
    let me: Option<UserSummary> = cache::get_json(&cache::keys::me()).await;
    let now = store::now_secs();
    let entry = JournalEntry {
        id: temp,
        book_uuid: input.book_uuid.clone(),
        author_id: me.as_ref().map(|u| u.id).unwrap_or_default(),
        author_name: me.map(|u| u.username).unwrap_or_default(),
        body_md: input.body_md.clone(),
        body_html: fallback_html(&input.body_md),
        progress: input.progress,
        status: input.status,
        client_id: None,
        created_at: now,
        updated_at: now,
    };
    let queued = enqueue(Op::CreateJournal {
        temp_id: temp,
        input: input.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    apply::journal_created(&entry).await;
    Some(entry)
}

/// Queue a journal-entry edit; returns the patched entry from cache.
pub(crate) async fn queue_update_journal(
    id: i64,
    input: &UpdateJournalEntry,
) -> Option<JournalEntry> {
    let book_uuid = apply::find_journal_book(id).await.unwrap_or_default();
    let queued = enqueue(Op::UpdateJournal {
        id,
        book_uuid: book_uuid.clone(),
        input: input.clone(),
    })
    .await;
    if !queued {
        return None;
    }
    apply::journal_updated(&book_uuid, id, input).await
}

/// Queue a journal-entry deletion (an unsynced temp entry just cancels).
pub(crate) async fn queue_delete_journal(id: i64) -> bool {
    let book_uuid = apply::find_journal_book(id).await.unwrap_or_default();
    if id < 0 {
        cancel_temp(id).await;
        apply::journal_deleted(&book_uuid, id).await;
        return true;
    }
    let queued = enqueue(Op::DeleteJournal {
        id,
        book_uuid: book_uuid.clone(),
    })
    .await;
    if queued {
        apply::journal_deleted(&book_uuid, id).await;
    }
    queued
}

/// Queue a Kindle-email change.
pub(crate) async fn queue_set_kindle_email(email: &Option<String>) -> bool {
    let queued = enqueue(Op::SetKindleEmail {
        email: email.clone(),
    })
    .await;
    if queued {
        apply::kindle_email_changed(email).await;
    }
    queued
}

/// Minimal offline stand-in for the server's markdown render: HTML-escape,
/// then split blank-line-separated paragraphs. Superseded by the real
/// render on the first post-drain refetch.
pub(crate) fn fallback_html(body_md: &str) -> String {
    let escaped = body_md
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    escaped
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", p.trim().replace('\n', "<br>")))
        .collect()
}

#[cfg(test)]
mod tests;
