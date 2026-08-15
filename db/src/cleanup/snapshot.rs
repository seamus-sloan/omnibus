//! Snapshot types serialized into `cleanup_log.snapshot_json` by
//! [`super::apply`] and replayed by [`super::undo`]. Kept separate from
//! `apply.rs` so the wire shape undo depends on is auditable in one place —
//! any field apply writes but undo doesn't read (or vice versa) shows up as
//! a single missing match arm here instead of being scattered across both.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// A cached `author_photos` row, base64-encoded so a manual upload's bytes
/// don't bloat `snapshot_json` as a JSON array of numbers (~4x the raw size).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct PhotoSnapshot {
    pub source: String,
    pub url: Option<String>,
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_b64: Option<String>,
}

impl PhotoSnapshot {
    /// Encode raw photo bytes for storage in the snapshot; `None` for a
    /// `'letter'` marker row, which carries no bytes.
    pub fn encode(bytes: Option<&[u8]>) -> Option<String> {
        bytes.map(|b| BASE64.encode(b))
    }

    /// Decode this snapshot's bytes back to raw form, for writing the
    /// `author_photos` row on undo/reconcile.
    pub fn decode(&self) -> Option<Vec<u8>> {
        self.bytes_b64
            .as_deref()
            .and_then(|s| BASE64.decode(s).ok())
    }
}

/// One book's link row to a merge source before the merge — `position` is
/// `Some` only for authors (series/tags have no such column).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BookLink {
    pub book_id: i64,
    pub position: Option<i64>,
}

/// The `entity_aliases` row a merge's `write_entity_alias` call is about to
/// overwrite, captured beforehand so undo can restore it exactly rather than
/// deleting a mapping that predates this merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AliasSnapshot {
    pub canonical_id: i64,
    pub created_at: i64,
}

/// One entity absorbed into the canonical id during a merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct MergedSource {
    pub id: i64,
    pub name: String,
    pub sort: Option<String>,
    pub links: Vec<BookLink>,
    /// Authors only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<PhotoSnapshot>,
    /// The name's `entity_aliases` row before this merge overwrote it —
    /// `None` if the name had never been merged away before. Undo restores
    /// this instead of unconditionally deleting the row, so a merge can't
    /// erase an earlier merge's alias mapping for the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_alias: Option<AliasSnapshot>,
}

/// Full pre-merge state for `apply_merge_authors`/`_series`/`_tags`, enough
/// for undo to recreate every absorbed row exactly as it was.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct MergeSnapshot {
    pub canonical_id: i64,
    pub canonical_sort_before: Option<String>,
    /// `true` when the merge backfilled a NULL `sort` on the canonical row
    /// — undo restores NULL rather than the backfilled value, instead of
    /// trusting `canonical_sort_before` alone (which is already `None` in
    /// that case and so can't distinguish "was NULL, stayed NULL" from
    /// "was NULL, got backfilled").
    pub canonical_sort_was_backfilled: bool,
    /// Authors only: the canonical's own photo row before reconciliation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_photo_before: Option<PhotoSnapshot>,
    /// Book ids the canonical entity was already linked to *before* the
    /// merge — undo needs this to tell "this link predates the merge,
    /// leave it" from "this link is the merge's INSERT OR IGNORE, remove
    /// it now that the source's own link is restored".
    pub canonical_book_ids_before: Vec<i64>,
    pub sources: Vec<MergedSource>,
}

/// Pre-split state for `apply_tag_split`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SplitSnapshot {
    pub source_name: String,
    pub delimiter: String,
    pub atoms: Vec<String>,
    /// Book ids linked to the source tag before the split.
    pub links: Vec<i64>,
}

/// Pre-rename state for `apply_book_title_override`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RenameSnapshot {
    pub book_uuid: String,
    /// The full previous `metadata_overrides.overrides` JSON blob, or
    /// `None` if the book had no override row before this rename — undo
    /// tells these apart by presence, not by an empty-vs-absent title.
    pub previous_overrides: Option<String>,
    pub previous_has_cover_override: bool,
}

/// Pre-delete state for `apply_delete_author`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DeleteAuthorSnapshot {
    pub name: String,
    pub sort: Option<String>,
    pub links: Vec<BookLink>,
    pub photo: Option<PhotoSnapshot>,
}
