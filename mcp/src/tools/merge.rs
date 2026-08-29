//! The book-merge tool family: merge one book into another and undo a
//! recorded merge. Both wrap the admin-gated REST endpoints
//! (`POST /api/books/merge`, `POST /api/books/merge/undo`) and refuse to
//! run without an explicit `confirm: true` — a merge deletes a `books` row
//! library-wide, the strongest write this crate exposes.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::Deserialize;

use omnibus_shared::{MergeBooksRequest, MergeBooksResult, UndoMergeRequest, UndoMergeResult};

use crate::client::ClientError;
use crate::server::OmnibusMcp;

/// Parameters for the merge: the two books plus the gate.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MergeParams {
    /// The book to absorb — its `books` row is deleted by the merge.
    pub source_uuid: String,
    /// The surviving book — it receives the source's files, links,
    /// identifiers, and per-reader state.
    pub target_uuid: String,
    /// Must be `true`. Omitting it (or passing `false`) refuses with an
    /// explanation of the fetch → present → confirm workflow.
    pub confirm: Option<bool>,
}

/// Parameters for the undo: the merge-log handle plus the gate.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UndoMergeParams {
    /// The `merge_log_id` a prior merge_books call returned — the undo
    /// handle for that specific merge.
    pub merge_log_id: i64,
    /// Must be `true`. Omitting it (or passing `false`) refuses with an
    /// explanation of the confirm workflow.
    pub confirm: Option<bool>,
}

/// The refusal both tools answer when `confirm` is not `true`.
fn unconfirmed(tool: &str, effect: &str, workflow: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!("refused: confirm is not true. {tool} {effect} — {workflow}"),
        None,
    )
}

/// Map a merge-route failure onto a model-actionable error: the routes are
/// admin-gated (not `can_edit`), so a 403 names the admin requirement, and
/// the semantic statuses (404 unknown uuid/log id, 409 already undone,
/// 422 same book) carry the server's own message verbatim.
fn merge_error(e: ClientError) -> ErrorData {
    match &e {
        ClientError::WriteStatus { status: 403, .. } => ErrorData::invalid_params(
            "forbidden: merging (and unmerging) books requires an admin account — the \
             signed-in user is not an admin, and no other permission flag (can_edit \
             included) grants it"
                .to_string(),
            None,
        ),
        ClientError::WriteStatus {
            status: status @ (404 | 409 | 422),
            message,
            ..
        } => ErrorData::invalid_params(format!("HTTP {status}: {message}"), None),
        _ => e.into(),
    }
}

#[tool_router(router = merge_tools, vis = "pub(crate)")]
impl OmnibusMcp {
    #[tool(
        description = "Merge one book into another (duplicate resolution): the target book absorbs the source's files, formats, identifiers, and every reader's per-book state (progress, ratings, highlights, bookmarks, journals), and the source book's row is DELETED — library-wide, every user sees the result. Admin-only. REFUSES unless confirm: true: first call get_book on both uuids, present both books' titles, authors, and formats to the user, and re-call with confirm: true only after the user approves merging that specific pair. Returns merge_log_id — the handle undo_merge needs to reverse this exact merge — plus the surviving target_uuid."
    )]
    pub async fn merge_books(
        &self,
        Parameters(p): Parameters<MergeParams>,
    ) -> Result<Json<MergeBooksResult>, ErrorData> {
        if p.confirm != Some(true) {
            return Err(unconfirmed(
                "merge_books",
                "deletes the source book's row and retargets every reader's state onto \
                 the target",
                "first call get_book on both uuids, present both books' titles, authors, \
                 and formats to the user, and only after the user approves re-call \
                 merge_books with the same source_uuid and target_uuid plus confirm: true",
            ));
        }
        let source = crate::tools::path_segment(&p.source_uuid, "source_uuid")?.to_string();
        let target = crate::tools::path_segment(&p.target_uuid, "target_uuid")?.to_string();
        let body = MergeBooksRequest {
            source_uuid: source,
            target_uuid: target,
        };
        let merged: MergeBooksResult = self
            .client
            .write_json(reqwest::Method::POST, "/api/books/merge", Some(&body))
            .await
            .map_err(merge_error)?;
        Ok(Json(merged))
    }

    #[tool(
        description = "Reverse a recorded merge: restores the source book merge_books deleted, using the merge_log_id that merge_books returned as the undo handle. Admin-only, library-wide like the merge itself. REFUSES unless confirm: true: tell the user which merge is being reversed (the merge_log_id and, via get_book on its target_uuid, the book it merged into) and re-call with confirm: true only after they approve. A merge can be undone once — a second undo of the same merge_log_id answers HTTP 409. Returns the restored (source) book's uuid."
    )]
    pub async fn undo_merge(
        &self,
        Parameters(p): Parameters<UndoMergeParams>,
    ) -> Result<Json<UndoMergeResult>, ErrorData> {
        if p.confirm != Some(true) {
            return Err(unconfirmed(
                "undo_merge",
                "restores the book a prior merge deleted",
                "tell the user which merge is being reversed (this merge_log_id, and the \
                 target book it merged into), and only after the user approves re-call \
                 undo_merge with the same merge_log_id plus confirm: true",
            ));
        }
        let body = UndoMergeRequest {
            merge_log_id: p.merge_log_id,
        };
        let restored: UndoMergeResult = self
            .client
            .write_json(reqwest::Method::POST, "/api/books/merge/undo", Some(&body))
            .await
            .map_err(merge_error)?;
        Ok(Json(restored))
    }
}

#[cfg(test)]
mod tests;
