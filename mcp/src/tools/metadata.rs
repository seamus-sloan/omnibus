//! The metadata repair tool family: dry-run diffs over the per-book override
//! endpoints, plus the provider-lookup reads that source repair values.
//! The workflow is propose (writes nothing) → show the user the diff →
//! apply/revert with an explicit `confirm: true`. Overrides are library-wide,
//! so every write is confirm-gated and takes an explicit uuid list only.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use omnibus_shared::metadata_lookup::{
    EditionHydrateRequest, EditionSearchRequest, EditionSearchResponse, ProviderEdition,
};
use omnibus_shared::{EbookMetadata, MetadataOverrides};

use crate::client::ClientError;
use crate::server::OmnibusMcp;

/// An explicit list of book uuids — the only addressing mode the write tools
/// accept, so the confirmed set is always the applied set.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookSetParams {
    /// The books' uuids (the `unique_identifier` field on book records).
    pub uuids: Vec<String>,
}

/// One book's proposed field changes, in the override wire shape.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookChange {
    /// The book's uuid.
    pub book_uuid: String,
    /// The fields to change. Only fields present are touched; list fields
    /// (`creators`, `subjects`, `genres`) replace the whole list, and an
    /// empty string clears a scalar field's override.
    pub changes: MetadataOverrides,
}

/// Parameters for the dry-run diff.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProposeParams {
    /// The per-book changes to preview.
    pub changes: Vec<BookChange>,
}

/// Parameters for the apply step — the propose change-set plus the gate.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyParams {
    /// The per-book changes to apply — the same list a prior
    /// propose_metadata_changes call diffed.
    pub changes: Vec<BookChange>,
    /// Must be `true`. Omitting it (or passing `false`) refuses with an
    /// explanation of the propose → confirm → apply workflow.
    pub confirm: Option<bool>,
}

/// Parameters for the revert step — an explicit uuid list plus the gate.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RevertParams {
    /// The books whose overrides to delete.
    pub uuids: Vec<String>,
    /// Must be `true`. Omitting it (or passing `false`) refuses with an
    /// explanation of the confirm workflow.
    pub confirm: Option<bool>,
}

/// One field's before/after in a dry-run diff.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FieldDiff {
    /// The override field name (e.g. `title`, `genres`).
    pub field: String,
    /// The book's current effective value (overrides already merged).
    pub before: serde_json::Value,
    /// The proposed value.
    pub after: serde_json::Value,
    /// Present when the change has semantics beyond a plain field edit —
    /// notably genres, which are override-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One book's dry-run diff.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BookDiff {
    pub book_uuid: String,
    /// The book's current title, for showing the diff to a person.
    pub title: Option<String>,
    /// Whether the book already carries metadata overrides. Applying any
    /// change establishes (or extends) an override row on the book.
    pub already_has_override: bool,
    pub fields: Vec<FieldDiff>,
}

/// The dry-run result: per-book diffs plus the workflow reminder.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProposedChanges {
    pub books: Vec<BookDiff>,
    /// What to do with this diff.
    pub next_step: String,
}

/// One book a write landed on.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WrittenBook {
    pub book_uuid: String,
    pub title: Option<String>,
    /// Whether the book carries overrides after the write (`true` after an
    /// apply, `false` after a revert).
    pub has_override: bool,
}

/// A fully-successful apply: every requested book was written.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApplyReport {
    pub applied: Vec<WrittenBook>,
}

/// A fully-successful revert: every requested book's overrides were deleted.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RevertReport {
    pub reverted: Vec<WrittenBook>,
}

/// The genre annotation AC5 requires: a genre change is not an override *of*
/// anything — it establishes the override row that is the genres' only home.
const GENRE_NOTE: &str =
    "genres are override-only (nothing Omnibus scans carries a genre), so this \
     change establishes a metadata override on the book and flips has_override; reverting the \
     book's overrides later clears its genres entirely rather than restoring a scanned value";

/// Serialize a value for a diff cell. Infallible for these wire types; a
/// hypothetical failure degrades to `null` rather than panicking.
fn jv<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// The per-field delta between a book's current effective metadata and one
/// proposed change-set. Only fields the change-set names appear.
fn diff_book(book: &EbookMetadata, changes: &MetadataOverrides) -> Vec<FieldDiff> {
    let mut fields = Vec::new();
    let mut push = |field: &str, before: serde_json::Value, after: serde_json::Value| {
        fields.push(FieldDiff {
            field: field.to_string(),
            before,
            after,
            note: None,
        });
    };
    let scalars: [(&str, &Option<String>, &Option<String>); 9] = [
        ("title", &changes.title, &book.title),
        ("description", &changes.description, &book.description),
        ("publisher", &changes.publisher, &book.publisher),
        ("published", &changes.published, &book.published),
        ("language", &changes.language, &book.language),
        ("series", &changes.series, &book.series),
        ("series_index", &changes.series_index, &book.series_index),
        ("isbn13", &changes.isbn13, &book.isbn13),
        ("isbn10", &changes.isbn10, &book.isbn10),
    ];
    for (name, proposed, current) in scalars {
        if let Some(after) = proposed {
            push(name, jv(current), jv(after));
        }
    }
    if let Some(after) = &changes.creators {
        push("creators", jv(&book.creators), jv(after));
    }
    if let Some(after) = &changes.subjects {
        push("subjects", jv(&book.subjects), jv(after));
    }
    if let Some(after) = &changes.print_pages {
        push("print_pages", jv(&book.print_pages), jv(after));
    }
    if let Some(after) = &changes.genres {
        fields.push(FieldDiff {
            field: "genres".to_string(),
            before: jv(&book.genres),
            after: jv(after),
            note: Some(GENRE_NOTE.to_string()),
        });
    }
    fields
}

/// The refusal both write tools answer when `confirm` is not `true`.
fn unconfirmed(tool: &str, effect: &str) -> ErrorData {
    ErrorData::invalid_params(
        format!(
            "refused: confirm is not true. {tool} {effect} — metadata overrides are \
             library-wide and visible to every user. First call propose_metadata_changes \
             (and get_effective_metadata) to preview the change, show the user the \
             per-book before/after diff, and only after the user approves re-call {tool} \
             with the same explicit book list plus confirm: true."
        ),
        None,
    )
}

/// Render a client failure on a `can_edit`-gated route: a 403 names the
/// missing permission instead of surfacing a bare status code.
fn describe_edit_failure(e: &ClientError) -> String {
    if matches!(
        e,
        ClientError::WriteStatus { status: 403, .. } | ClientError::Status { status: 403, .. }
    ) {
        return "the signed-in account lacks the edit permission (can_edit) this metadata \
                operation requires — ask an Omnibus admin to grant it"
            .to_string();
    }
    e.to_string()
}

/// Validate one change entry locally, so a dry run surfaces the same 400 the
/// server would and an apply cannot fail validation mid-batch.
fn validate_change(change: &BookChange) -> Result<(), ErrorData> {
    if change.changes == MetadataOverrides::default() {
        return Err(ErrorData::invalid_params(
            format!("book {}: changes names no fields", change.book_uuid),
            None,
        ));
    }
    change
        .changes
        .validate()
        .map_err(|msg| ErrorData::invalid_params(format!("book {}: {msg}", change.book_uuid), None))
}

/// The mid-batch failure report: which books were written, which failed,
/// which were never attempted. Partial application must be visible. The
/// structured payload keys are operation-neutral (`written`), since both the
/// apply and the revert batches report through here.
fn partial_failure(
    op: &str,
    written_verb: &str,
    written: &[WrittenBook],
    failed_uuid: &str,
    error: &ClientError,
    not_attempted: &[String],
) -> ErrorData {
    let written_uuids: Vec<&str> = written.iter().map(|b| b.book_uuid.as_str()).collect();
    let cause = describe_edit_failure(error);
    let message = format!(
        "{op} stopped partway: {} of {} books were {written_verb} before {failed_uuid} \
         failed ({cause}). {written_verb} (these writes DID land): {written_uuids:?}. Not \
         {written_verb}: {failed_uuid} plus {not_attempted:?}. Resolve the failure, then \
         re-run with only the books that were not {written_verb}.",
        written.len(),
        written.len() + 1 + not_attempted.len(),
    );
    ErrorData::internal_error(
        message,
        Some(serde_json::json!({
            "written": written_uuids,
            "failed": { "book_uuid": failed_uuid, "error": cause },
            "not_attempted": not_attempted,
        })),
    )
}

impl OmnibusMcp {
    /// One book's current effective metadata, or an error naming the uuid.
    async fn fetch_book(&self, uuid: &str) -> Result<EbookMetadata, ErrorData> {
        let uuid = crate::tools::path_segment(uuid, "uuid")?;
        let path = format!("/api/ebooks/{uuid}");
        let book: Option<EbookMetadata> = self.client.get_json_opt(&path, &[]).await?;
        book.ok_or_else(|| ErrorData::invalid_params(format!("book {uuid} not found"), None))
    }

    /// One book's write, mapped to the batch-failure vocabulary: `Ok` is the
    /// updated book, `Err` is the failure `partial_failure` will report.
    async fn write_book(
        &self,
        method: reqwest::Method,
        uuid: &str,
        body: Option<&MetadataOverrides>,
    ) -> Result<WrittenBook, ClientError> {
        let path = format!("/api/ebooks/{uuid}/overrides");
        let book: EbookMetadata = self.client.write_json(method, &path, body).await?;
        Ok(WrittenBook {
            book_uuid: uuid.to_string(),
            title: book.title,
            has_override: book.has_override,
        })
    }
}

#[tool_router(router = metadata_tools, vis = "pub(crate)")]
impl OmnibusMcp {
    #[tool(
        description = "Current effective metadata (scanned values with any overrides already merged) for an explicit list of book uuids — the before-state that metadata repair works from. Step 1 of the repair workflow: fetch this, then call propose_metadata_changes for a dry-run diff, show the user, and only then apply_metadata_changes with confirm: true. Errors if any uuid is unknown."
    )]
    pub async fn get_effective_metadata(
        &self,
        Parameters(p): Parameters<BookSetParams>,
    ) -> Result<Json<Vec<EbookMetadata>>, ErrorData> {
        let mut books = Vec::with_capacity(p.uuids.len());
        for uuid in &p.uuids {
            books.push(self.fetch_book(uuid).await?);
        }
        Ok(Json(books))
    }

    #[tool(
        description = "DRY RUN — writes nothing. Takes per-book field changes ({book_uuid, changes}) and returns a per-book before/after diff computed against current effective metadata. changes uses the override shape: only fields present are touched, list fields (creators, subjects/tags, genres) replace the whole list, and an empty string clears a scalar. A genres entry is annotated specially: genres are override-only, so a genre change establishes a metadata override on the book. Always call this first, show the user the diff, then call apply_metadata_changes with the same change list and confirm: true."
    )]
    pub async fn propose_metadata_changes(
        &self,
        Parameters(p): Parameters<ProposeParams>,
    ) -> Result<Json<ProposedChanges>, ErrorData> {
        if p.changes.is_empty() {
            return Err(ErrorData::invalid_params("changes names no books", None));
        }
        let mut books = Vec::with_capacity(p.changes.len());
        for change in &p.changes {
            crate::tools::path_segment(&change.book_uuid, "book_uuid")?;
            validate_change(change)?;
            let book = self.fetch_book(&change.book_uuid).await?;
            books.push(BookDiff {
                book_uuid: change.book_uuid.clone(),
                title: book.title.clone(),
                already_has_override: book.has_override,
                fields: diff_book(&book, &change.changes),
            });
        }
        Ok(Json(ProposedChanges {
            books,
            next_step: "Nothing has been written. Show the user this per-book diff; once they \
                        approve, call apply_metadata_changes with the same changes plus \
                        confirm: true."
                .to_string(),
        }))
    }

    #[tool(
        description = "Apply per-book metadata changes as overrides (one write per book). Overrides are library-wide — every user sees the result — so this tool REFUSES unless confirm: true: first call propose_metadata_changes, show the user the diff, and pass confirm: true only after the user approves. Applies exactly the explicit book list given (there is deliberately no filter/query form) sequentially; on a mid-batch failure it stops, and the error reports which books were written and which were not. Requires the account's edit permission (can_edit)."
    )]
    pub async fn apply_metadata_changes(
        &self,
        Parameters(p): Parameters<ApplyParams>,
    ) -> Result<Json<ApplyReport>, ErrorData> {
        if p.confirm != Some(true) {
            return Err(unconfirmed(
                "apply_metadata_changes",
                "writes metadata overrides onto every listed book",
            ));
        }
        if p.changes.is_empty() {
            return Err(ErrorData::invalid_params("changes names no books", None));
        }
        // Everything checkable locally is checked before the first write, so
        // a validation problem can never leave the batch half-applied.
        for change in &p.changes {
            crate::tools::path_segment(&change.book_uuid, "book_uuid")?;
            validate_change(change)?;
        }
        let mut applied: Vec<WrittenBook> = Vec::with_capacity(p.changes.len());
        for (i, change) in p.changes.iter().enumerate() {
            match self
                .write_book(
                    reqwest::Method::POST,
                    &change.book_uuid,
                    Some(&change.changes),
                )
                .await
            {
                Ok(book) => applied.push(book),
                Err(e) if applied.is_empty() => {
                    return Err(ErrorData::internal_error(
                        format!(
                            "apply_metadata_changes wrote nothing: {} failed ({})",
                            change.book_uuid,
                            describe_edit_failure(&e)
                        ),
                        None,
                    ));
                }
                Err(e) => {
                    let not_attempted: Vec<String> = p.changes[i + 1..]
                        .iter()
                        .map(|c| c.book_uuid.clone())
                        .collect();
                    return Err(partial_failure(
                        "apply_metadata_changes",
                        "applied",
                        &applied,
                        &change.book_uuid,
                        &e,
                        &not_attempted,
                    ));
                }
            }
        }
        Ok(Json(ApplyReport { applied }))
    }

    #[tool(
        description = "Delete every metadata override on each listed book, restoring the file-embedded scanned metadata (one delete per book). Override-only fields — genres, print_pages, isbn10 — are cleared outright, since they have no scanned value underneath. Library-wide, so this tool REFUSES unless confirm: true: show the user the affected books (get_effective_metadata reports current values and has_override) and pass confirm: true only after they approve the explicit uuid list. Sequential with the same stop-and-report batch semantics as apply_metadata_changes. Requires can_edit."
    )]
    pub async fn revert_metadata_overrides(
        &self,
        Parameters(p): Parameters<RevertParams>,
    ) -> Result<Json<RevertReport>, ErrorData> {
        if p.confirm != Some(true) {
            return Err(unconfirmed(
                "revert_metadata_overrides",
                "deletes every metadata override on the listed books, including \
                 override-only genres",
            ));
        }
        if p.uuids.is_empty() {
            return Err(ErrorData::invalid_params("uuids names no books", None));
        }
        for uuid in &p.uuids {
            crate::tools::path_segment(uuid, "uuid")?;
        }
        let mut reverted: Vec<WrittenBook> = Vec::with_capacity(p.uuids.len());
        for (i, uuid) in p.uuids.iter().enumerate() {
            match self
                .write_book(reqwest::Method::DELETE, uuid, None::<&MetadataOverrides>)
                .await
            {
                Ok(book) => reverted.push(book),
                Err(e) if reverted.is_empty() => {
                    return Err(ErrorData::internal_error(
                        format!(
                            "revert_metadata_overrides reverted nothing: {uuid} failed ({})",
                            describe_edit_failure(&e)
                        ),
                        None,
                    ));
                }
                Err(e) => {
                    let not_attempted: Vec<String> = p.uuids[i + 1..].to_vec();
                    return Err(partial_failure(
                        "revert_metadata_overrides",
                        "reverted",
                        &reverted,
                        uuid,
                        &e,
                        &not_attempted,
                    ));
                }
            }
        }
        Ok(Json(RevertReport { reverted }))
    }

    #[tool(
        description = "Search the external metadata providers (Open Library, Google Books, and Hardcover when configured) for candidate editions of a book — the source of correct values for metadata repair. This is a read: it writes nothing (the POST is read-shaped), but the server requires the edit permission (can_edit) because it spends outbound provider calls. Provide free-text query and/or structured title/author/isbn (the ISBN is the strongest signal). Candidates stay attributed per provider, alongside a per-source status row; feed a candidate's source + provider_ref to hydrate_provider_edition for its full record."
    )]
    pub async fn search_metadata_providers(
        &self,
        Parameters(p): Parameters<EditionSearchRequest>,
    ) -> Result<Json<EditionSearchResponse>, ErrorData> {
        let found: EditionSearchResponse = self
            .client
            .write_json(
                reqwest::Method::POST,
                "/api/metadata/editions/search",
                Some(&p),
            )
            .await
            .map_err(|e| ErrorData::internal_error(describe_edit_failure(&e), None))?;
        Ok(Json(found))
    }

    #[tool(
        description = "Re-fetch one selected search candidate in full from the provider that offered it — a search hit is thinner than the provider's own record (e.g. Open Library search hits carry no description). Pass the candidate's source and provider_ref (and isbn13 when it has one) from search_metadata_providers. A read; requires can_edit like the search. Returns null when the provider no longer knows the candidate."
    )]
    pub async fn hydrate_provider_edition(
        &self,
        Parameters(p): Parameters<EditionHydrateRequest>,
    ) -> Result<Json<Option<ProviderEdition>>, ErrorData> {
        let found: Option<ProviderEdition> = self
            .client
            .write_json(
                reqwest::Method::POST,
                "/api/metadata/editions/hydrate",
                Some(&p),
            )
            .await
            .map_err(|e| ErrorData::internal_error(describe_edit_failure(&e), None))?;
        Ok(Json(found))
    }
}

#[cfg(test)]
mod tests;
