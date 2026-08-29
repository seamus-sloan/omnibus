//! The shelf authoring tool family: create, update, and delete shelves,
//! edit hand-picked membership, and preview an unsaved smart rule. Every
//! tool wraps one existing `/api/shelves*` endpoint on the
//! [`crate::client::WRITE_ALLOWLIST`]; descriptions are the model-facing
//! API docs — the rule syntax documented on `preview_shelf_rule` is the
//! source of truth for what a model may send.

use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use omnibus_shared::{
    CreateShelfRequest, MatchMode, RulePreview, RulePreviewRequest, Shelf, ShelfKind, ShelfRule,
    UpdateShelfRequest, Visibility,
};

use crate::client::ClientError;
use crate::server::OmnibusMcp;

/// Parameters for previewing a candidate smart-shelf rule set.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PreviewRuleParams {
    /// How the rules combine: `all` = AND, `any` = OR.
    pub match_mode: MatchMode,
    /// The candidate conditions (at most 50).
    pub rules: Vec<ShelfRule>,
}

/// Parameters for creating a shelf.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateShelfParams {
    /// `manual` (hand-picked members) or `smart` (rule-driven membership).
    pub kind: ShelfKind,
    /// Shelf name, unique per library (≤ 120 chars).
    pub name: String,
    /// Optional description (≤ 2048 chars).
    pub description: Option<String>,
    /// `private` (owner + admins, the default) or `public` (every user).
    pub visibility: Option<Visibility>,
    /// Required for smart shelves: how the rules combine.
    pub match_mode: Option<MatchMode>,
    /// Required (non-empty) for smart shelves, forbidden for manual ones.
    pub rules: Option<Vec<ShelfRule>>,
    /// Optional initial members for a manual shelf (book uuids).
    pub book_uuids: Option<Vec<String>>,
}

/// Parameters for partially updating a shelf.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateShelfParams {
    /// Shelf id from list_shelves / get_shelf.
    pub id: i64,
    /// New name (≤ 120 chars, unique per library).
    pub name: Option<String>,
    /// New description (≤ 2048 chars).
    pub description: Option<String>,
    /// `private` (owner + admins) or `public` (every user).
    pub visibility: Option<Visibility>,
    /// New match mode (smart shelves only).
    pub match_mode: Option<MatchMode>,
    /// Replaces the ENTIRE rule set (smart shelves only) — send the full
    /// final set, not a delta.
    pub rules: Option<Vec<ShelfRule>>,
    /// Opt the shelf's books in or out of the owner's Kobo wireless sync.
    pub sync_to_kobo: Option<bool>,
}

/// Parameters for appending books to a hand-picked shelf.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddBooksParams {
    /// Shelf id from list_shelves / get_shelf.
    pub id: i64,
    /// Book uuids to append (from the listing/search tools; at most 2000).
    pub book_uuids: Vec<String>,
}

/// Parameters for removing one book from a shelf.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RemoveBookParams {
    /// Shelf id from list_shelves / get_shelf.
    pub id: i64,
    /// The member book's uuid.
    pub uuid: String,
}

/// Parameters for deleting a shelf.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteShelfParams {
    /// Shelf id from list_shelves / get_shelf.
    pub id: i64,
    /// Must be `true`. Deletion is permanent; set it only after the user has
    /// explicitly confirmed deleting this specific shelf.
    #[serde(default)]
    pub confirm: bool,
}

/// Acknowledgement for the shelf writes whose endpoint answers
/// `204 No Content` on success.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WriteAck {
    /// Always `true` — an unsuccessful write is a tool error instead.
    pub done: bool,
}

/// Map a shelf-write failure onto a model-actionable error: 403 names the
/// permission rule, and the other 4xx errors carry the server's own message
/// (rule validation, name conflicts) verbatim so the model can self-correct.
fn write_error(e: ClientError) -> ErrorData {
    match &e {
        ClientError::WriteStatus {
            status: 403,
            message,
            ..
        } => ErrorData::invalid_params(
            format!(
                "forbidden: only a shelf's owner (or an admin) may modify it, and \
                 system shelves like the built-in Wishlist can never be modified — \
                 server said: {message}"
            ),
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

#[tool_router(router = shelf_tools, vis = "pub(crate)")]
impl OmnibusMcp {
    #[tool(
        description = "Evaluate a candidate smart-shelf rule set WITHOUT creating anything: returns how many books match (matched of total) plus a sample of the matching books. This is the iteration tool — preview, inspect the sample, refine the rules, preview again, and only call create_shelf once the preview matches the intent, passing the exact rules you previewed. Rule syntax: Each rule is {field, op, value} (all strings); rules combine per match_mode: 'all' = every rule must hold (AND), 'any' = at least one (OR). Fields and their operators: tag / genre / author / series take is, is_not, contains, starts_with with value = the name, case-insensitive (genres are user-assigned; tags come from the files); format takes includes (exact, e.g. 'epub', 'cbz', 'm4b', 'mp3'), contains, starts_with; rating takes is or gte with value = stars '0.5' to '5' (halves allowed, e.g. '3.5'); status takes is with value 'unread', 'reading', or 'finished'; year takes is or gte with an integer publication year; date_added / date_updated take in_last (value '30d', '2w', '3m', '1y'), between (value 'START..END' ISO dates, e.g. '2025-01-01..2025-06-30'), before, after (ISO date). rating and status evaluate against the shelf owner's data. Example: {\"match_mode\": \"all\", \"rules\": [{\"field\": \"author\", \"op\": \"contains\", \"value\": \"le guin\"}, {\"field\": \"format\", \"op\": \"includes\", \"value\": \"epub\"}]} matches Le Guin books that have an EPUB file."
    )]
    pub async fn preview_shelf_rule(
        &self,
        Parameters(p): Parameters<PreviewRuleParams>,
    ) -> Result<Json<RulePreview>, ErrorData> {
        let req = RulePreviewRequest {
            match_mode: p.match_mode,
            rules: p.rules,
        };
        let preview: RulePreview = self
            .client
            .write_json(Method::POST, "/api/shelves/preview", &req)
            .await
            .map_err(write_error)?;
        Ok(Json(preview))
    }

    #[tool(
        description = "Create a shelf owned by the signed-in user and return it. kind 'manual' takes an optional book_uuids member list and no rules; kind 'smart' requires match_mode + rules (same syntax as preview_shelf_rule — ALWAYS iterate the rule with preview_shelf_rule first, then create with the exact rules you previewed). Names must be unique. visibility defaults to private (owner + admins only)."
    )]
    pub async fn create_shelf(
        &self,
        Parameters(p): Parameters<CreateShelfParams>,
    ) -> Result<Json<Shelf>, ErrorData> {
        let req = CreateShelfRequest {
            kind: p.kind,
            name: p.name,
            description: p.description,
            visibility: p.visibility.unwrap_or_default(),
            match_mode: p.match_mode,
            rules: p.rules.unwrap_or_default(),
            book_uuids: p.book_uuids.unwrap_or_default(),
        };
        // Enforce the kind-specific invariants (smart needs match_mode +
        // rules, manual takes neither, the system Wishlist kind is never
        // creatable) locally, so the model gets the message without a
        // round-trip the server would 422.
        req.validate()
            .map_err(|msg| ErrorData::invalid_params(msg, None))?;
        let shelf: Shelf = self
            .client
            .write_json(Method::POST, "/api/shelves", &req)
            .await
            .map_err(write_error)?;
        Ok(Json(shelf))
    }

    #[tool(
        description = "Partially update a shelf and return it: only the fields you pass change, and rules (smart shelves only) replaces the whole rule set — preview the new set with preview_shelf_rule first. Only the shelf's owner (or an admin) may update it: a shelf you can see but don't own answers 403, and the built-in Wishlist shelf can never be modified."
    )]
    pub async fn update_shelf(
        &self,
        Parameters(p): Parameters<UpdateShelfParams>,
    ) -> Result<Json<Shelf>, ErrorData> {
        let req = UpdateShelfRequest {
            name: p.name,
            description: p.description,
            visibility: p.visibility,
            match_mode: p.match_mode,
            rules: p.rules,
            sync_to_kobo: p.sync_to_kobo,
        };
        let path = format!("/api/shelves/{}", p.id);
        let shelf: Shelf = self
            .client
            .write_json(Method::PATCH, &path, &req)
            .await
            .map_err(write_error)?;
        Ok(Json(shelf))
    }

    #[tool(
        description = "Append books (by uuid, from the listing/search tools) to a hand-picked manual shelf. Owner-or-admin only (403 otherwise); smart shelves compute their membership from rules and cannot take hand-picked books."
    )]
    pub async fn add_books_to_shelf(
        &self,
        Parameters(p): Parameters<AddBooksParams>,
    ) -> Result<Json<WriteAck>, ErrorData> {
        let path = format!("/api/shelves/{}/books", p.id);
        let body = serde_json::json!({ "book_uuids": p.book_uuids });
        self.client
            .write_no_content(Method::POST, &path, Some(&body))
            .await
            .map_err(write_error)?;
        Ok(Json(WriteAck { done: true }))
    }

    #[tool(
        description = "Remove one book (by uuid) from a hand-picked manual shelf. Owner-or-admin only (403 otherwise). The book itself is untouched — only the shelf membership is removed."
    )]
    pub async fn remove_book_from_shelf(
        &self,
        Parameters(p): Parameters<RemoveBookParams>,
    ) -> Result<Json<WriteAck>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.uuid, "uuid")?;
        let path = format!("/api/shelves/{}/books/{uuid}", p.id);
        self.client
            .write_no_content::<()>(Method::DELETE, &path, None)
            .await
            .map_err(write_error)?;
        Ok(Json(WriteAck { done: true }))
    }

    #[tool(
        description = "Delete a shelf permanently — the one destructive tool in this family. Requires confirm: true, which you must only set after the user has explicitly confirmed deleting this specific shelf (name it to them first). Owner-or-admin only (403 otherwise); the built-in Wishlist shelf cannot be deleted. Member books are untouched."
    )]
    pub async fn delete_shelf(
        &self,
        Parameters(p): Parameters<DeleteShelfParams>,
    ) -> Result<Json<WriteAck>, ErrorData> {
        if !p.confirm {
            return Err(ErrorData::invalid_params(
                "refusing to delete: shelf deletion is permanent. Confirm the deletion \
                 of this specific shelf with the user (by name), then call delete_shelf \
                 again with confirm: true.",
                None,
            ));
        }
        let path = format!("/api/shelves/{}", p.id);
        self.client
            .write_no_content::<()>(Method::DELETE, &path, None)
            .await
            .map_err(write_error)?;
        Ok(Json(WriteAck { done: true }))
    }
}
