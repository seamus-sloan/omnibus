//! The check-in / wishlist / physical-collection tool family: resolve ISBNs
//! and title searches down the server's matching ladder, file physical
//! copies, and manage the caller's wishlist and the library's copy records.
//! Every mutation maps to a [`crate::client::WRITE_ALLOWLIST`] entry; the two
//! irreversible tools (check-in's ISBN binding, copy deletion) are gated on
//! an explicit `confirm` argument.

use reqwest::Method;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use omnibus_shared::{
    BookRef, CheckInRequest, ExternalBookMeta, PhysicalCopy, ResolveMetaRequest, ResolveRequest,
    ScanOutcome, ScanSearchRequest, ScanSearchResponse, UpdateCopyNoteRequest, WishlistAddRequest,
    WishlistSource,
};

use crate::client::ClientError;
use crate::server::OmnibusMcp;

/// A single book handle for the physical-collection tools — the
/// `unique_identifier` from the listing/search tools, or the `uuid` on a
/// scan outcome's matched book.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookUuid {
    /// The book's uuid.
    pub uuid: String,
}

/// Parameters for the batch ISBN lookup.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupIsbnParams {
    /// ISBNs to resolve (ISBN-10 or ISBN-13; separators tolerated). Each is
    /// one server-side lookup, run sequentially — the endpoint has no batch
    /// form.
    pub isbns: Vec<String>,
}

/// One ISBN's trip down the resolution ladder.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IsbnResolution {
    /// The ISBN as submitted.
    pub isbn: String,
    /// The ladder's outcome; absent when the lookup request itself failed
    /// (invalid ISBN, providers unreachable) — see `detail`.
    pub outcome: Option<ScanOutcome>,
    /// The external provider that supplied the metadata, when the outcome
    /// carries provider-resolved metadata (`close_match` / `not_in_library`,
    /// via that metadata's `source` field). Absent otherwise: exact-identifier
    /// library hits are answered by the library itself, and `unresolved`
    /// means no provider answered at all.
    pub provider: Option<String>,
    /// Human-readable explanation — why an `unresolved` outcome had no
    /// match, how a library hit was matched, or the server's error message
    /// for a failed lookup.
    pub detail: Option<String>,
}

/// Parameters for the provider title search.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMetadataParams {
    /// Title text (optionally with the author) to search the providers for.
    pub query: String,
}

/// Parameters for resolving a picked search candidate against the library.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveMetadataParams {
    /// A candidate exactly as returned by `search_book_metadata`.
    pub meta: ExternalBookMeta,
}

/// Parameters for filing a physical copy.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckInParams {
    /// The library book's uuid, from a `lookup_isbn` /
    /// `resolve_book_metadata` outcome the user has confirmed.
    pub book_uuid: String,
    /// The ISBN to record on the copy — and permanently bind to this book
    /// for future exact-identifier lookups.
    pub isbn: Option<String>,
    /// Free-text edition/condition note for the copy.
    pub note: Option<String>,
    /// Must be `true`. Set it only after showing the user the resolved book
    /// and getting their go-ahead.
    #[serde(default)]
    pub confirm: bool,
}

/// Parameters for the wishlist add.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddToWishlistParams {
    /// Uuid of a book already in the library. Set this or `meta`, not both;
    /// when both are present the uuid wins and `meta` is ignored.
    pub uuid: Option<String>,
    /// External metadata for a book the library does not hold (from
    /// `search_book_metadata` or a `not_in_library` lookup outcome) —
    /// creates a fileless wishlist book.
    pub meta: Option<ExternalBookMeta>,
}

/// Parameters for the copy-note edit.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateCopyNoteParams {
    /// The copy's `id` from `list_physical_copies`.
    pub copy_id: i64,
    /// Replacement note; omit (or send blank) to clear it.
    pub note: Option<String>,
}

/// Parameters for deleting a physical copy.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RemoveCopyParams {
    /// The copy's `id` from `list_physical_copies`.
    pub copy_id: i64,
    /// Must be `true`. Set it only after the user has confirmed which copy
    /// to delete — the deletion is permanent.
    #[serde(default)]
    pub confirm: bool,
}

/// Acknowledgement for tools whose endpoint answers `204 No Content`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Ack {
    /// What happened, in words.
    pub message: String,
}

/// The provider named by a provider-resolved outcome's metadata; library
/// hits and `unresolved` carry none.
fn provider_of(outcome: &ScanOutcome) -> Option<String> {
    match outcome {
        ScanOutcome::CloseMatch { scanned, .. } => Some(scanned.source.display_name().to_string()),
        ScanOutcome::NotInLibrary { online } => Some(online.source.display_name().to_string()),
        _ => None,
    }
}

/// Attach the honest provenance story to an outcome: who answered, or why
/// nobody did.
fn resolution(isbn: String, outcome: ScanOutcome) -> IsbnResolution {
    let provider = provider_of(&outcome);
    let detail = match &outcome {
        ScanOutcome::AlreadyOwned { .. }
        | ScanOutcome::OnWishlist { .. }
        | ScanOutcome::InLibraryUnowned { .. } => Some(
            "Matched a library book by exact identifier — no external provider was consulted."
                .to_string(),
        ),
        ScanOutcome::CloseMatch { .. } | ScanOutcome::NotInLibrary { .. } => None,
        ScanOutcome::Unresolved => Some(
            "Neither the library nor any reachable metadata provider recognized this ISBN. \
             The provider ladder is API-key-dependent (a provider without a configured key is \
             skipped), so this instance may have consulted fewer sources; try \
             search_book_metadata with the title instead."
                .to_string(),
        ),
    };
    IsbnResolution {
        isbn,
        outcome: Some(outcome),
        provider,
        detail,
    }
}

/// Map a write failure to a tool error, naming the missing permission on a
/// 403 and passing the server's actionable message through on the other
/// user-addressable statuses.
fn write_error(e: ClientError) -> ErrorData {
    match e {
        ClientError::WriteStatus {
            status: 403,
            message,
            ..
        } => ErrorData::invalid_params(
            format!(
                "forbidden: {message} — the signed-in account lacks the `can_edit` permission \
                 (physical copies are library-wide, so editing them takes the same gate as \
                 metadata overrides; an admin can grant it)"
            ),
            None,
        ),
        ClientError::WriteStatus {
            status: status @ (400 | 404 | 409),
            message,
            ..
        } => ErrorData::invalid_params(format!("HTTP {status}: {message}"), None),
        other => other.into(),
    }
}

#[tool_router(router = checkin_tools, vis = "pub(crate)")]
impl OmnibusMcp {
    #[tool(
        description = "Resolve one or more ISBNs down the check-in ladder: exact identifier match against the library first, then the external metadata providers. Each result reports which provider answered when metadata came from a provider (results are provider-order- and API-key-dependent, so name the provider when relaying them); exact library hits involve no provider, and an unresolved ISBN is reported with an explanation rather than dropped. Outcomes: already_owned / on_wishlist / in_library_unowned (library hits), close_match (provider-resolved, fuzzily matched to library books — needs human confirmation), not_in_library (provider-resolved only), unresolved. To file a physical copy afterwards, show the user the matched book, then call check_in_physical_book with confirm=true."
    )]
    pub async fn lookup_isbn(
        &self,
        Parameters(p): Parameters<LookupIsbnParams>,
    ) -> Result<Json<Vec<IsbnResolution>>, ErrorData> {
        if p.isbns.is_empty() {
            return Err(ErrorData::invalid_params(
                "isbns must contain at least one ISBN",
                None,
            ));
        }
        let mut results = Vec::with_capacity(p.isbns.len());
        for isbn in p.isbns {
            let req = ResolveRequest { isbn: isbn.clone() };
            match self
                .client
                .write_json::<_, ScanOutcome>(Method::POST, "/api/scan/resolve", &req)
                .await
            {
                Ok(outcome) => results.push(resolution(isbn, outcome)),
                // A per-ISBN failure (invalid ISBN, providers down) is part
                // of the batch answer, not a reason to lose the other rows.
                Err(ClientError::WriteStatus {
                    status, message, ..
                }) => results.push(IsbnResolution {
                    isbn,
                    outcome: None,
                    provider: None,
                    detail: Some(format!("lookup failed (HTTP {status}): {message}")),
                }),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Json(results))
    }

    #[tool(
        description = "Search the external metadata providers by title/author text — the fallback when lookup_isbn reports unresolved. Returns candidate editions, each carrying `source` (which provider reported it) and the isbn13 it carries; which providers answer depends on the instance's configured API keys. Feed a picked candidate to resolve_book_metadata to match it against the library before acting on it."
    )]
    pub async fn search_book_metadata(
        &self,
        Parameters(p): Parameters<SearchMetadataParams>,
    ) -> Result<Json<ScanSearchResponse>, ErrorData> {
        let req = ScanSearchRequest { query: p.query };
        Ok(Json(
            self.client
                .write_json(Method::POST, "/api/scan/search", &req)
                .await
                .map_err(write_error)?,
        ))
    }

    #[tool(
        description = "Match a picked search_book_metadata candidate against the library — the ladder's library rungs applied to metadata already in hand, no provider round-trip. Returns the same per-ISBN resolution shape as lookup_isbn; the provider named is the one the candidate came from."
    )]
    pub async fn resolve_book_metadata(
        &self,
        Parameters(p): Parameters<ResolveMetadataParams>,
    ) -> Result<Json<IsbnResolution>, ErrorData> {
        let isbn = p.meta.isbn13.clone();
        let req = ResolveMetaRequest { meta: p.meta };
        let outcome: ScanOutcome = self
            .client
            .write_json(Method::POST, "/api/scan/resolve-meta", &req)
            .await
            .map_err(write_error)?;
        Ok(Json(resolution(isbn, outcome)))
    }

    #[tool(
        description = "File a physical copy of a library book. Two effects, one of them permanent: a copy row is created (visible to every user on the book's detail page), AND the given ISBN is bound to this book on the exact-identifier rung, so that ISBN resolves to this book in every future lookup. Resolve first (lookup_isbn / resolve_book_metadata), show the user the matched book, and only call this with confirm=true after they approve. Refuses without confirm."
    )]
    pub async fn check_in_physical_book(
        &self,
        Parameters(p): Parameters<CheckInParams>,
    ) -> Result<Json<BookRef>, ErrorData> {
        if !p.confirm {
            return Err(ErrorData::invalid_params(
                "confirm must be true: check-in files a library-wide physical copy, and when \
                 an isbn is passed it is permanently bound to this book for future \
                 exact-identifier lookups. Show the user the matched book, and pass \
                 confirm=true once they approve.",
                None,
            ));
        }
        let req = CheckInRequest {
            book_uuid: p.book_uuid,
            isbn: p.isbn,
            note: p.note,
        };
        Ok(Json(
            self.client
                .write_json(Method::POST, "/api/scan/check-in", &req)
                .await
                .map_err(write_error)?,
        ))
    }

    #[tool(
        description = "Add a book to the signed-in user's physical wishlist: pass `uuid` for a book already in the library, or `meta` (a search_book_metadata candidate / not_in_library lookup result) to create a fileless wishlist book the library doesn't hold. Idempotent for an already-wishlisted book. Returns the uuid of the book the entry landed on."
    )]
    pub async fn add_to_wishlist(
        &self,
        Parameters(p): Parameters<AddToWishlistParams>,
    ) -> Result<Json<BookRef>, ErrorData> {
        if p.uuid.is_none() && p.meta.is_none() {
            return Err(ErrorData::invalid_params(
                "pass uuid (library book) or meta (external candidate)",
                None,
            ));
        }
        // Provenance the flow can actually claim: a uuid means the book was
        // already identified in the library (the detail-page path); meta
        // means it came out of a provider lookup/search. Never `scan` — no
        // camera is involved here.
        let source = if p.uuid.is_some() {
            WishlistSource::Detail
        } else {
            WishlistSource::Search
        };
        let req = WishlistAddRequest {
            book_uuid: p.uuid,
            meta: p.meta,
            source,
        };
        Ok(Json(
            self.client
                .write_json(Method::POST, "/api/scan/wishlist", &req)
                .await
                .map_err(write_error)?,
        ))
    }

    #[tool(
        description = "Remove a book from the signed-in user's physical wishlist by uuid. Idempotent — removing a book that isn't wishlisted still succeeds, since the desired end state already holds."
    )]
    pub async fn remove_from_wishlist(
        &self,
        Parameters(p): Parameters<BookUuid>,
    ) -> Result<Json<Ack>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.uuid, "uuid")?;
        let path = format!("/api/physical/{uuid}/wishlist");
        self.client
            .write_no_content::<()>(Method::DELETE, &path, None)
            .await
            .map_err(write_error)?;
        Ok(Json(Ack {
            message: format!("removed {uuid} from the wishlist"),
        }))
    }

    #[tool(
        description = "List a book's physical copies (library-wide, oldest check-in first), each with its id, recorded ISBN, check-in time, and note. An unknown uuid simply has no copies (empty list)."
    )]
    pub async fn list_physical_copies(
        &self,
        Parameters(p): Parameters<BookUuid>,
    ) -> Result<Json<Vec<PhysicalCopy>>, ErrorData> {
        let path = format!("/api/physical/{}/copies", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }

    #[tool(
        description = "Replace a physical copy's free-text edition/condition note (blank or omitted note clears it). Copies are library-wide, so this requires the `can_edit` permission. Returns the updated copy."
    )]
    pub async fn update_copy_note(
        &self,
        Parameters(p): Parameters<UpdateCopyNoteParams>,
    ) -> Result<Json<PhysicalCopy>, ErrorData> {
        let path = format!("/api/physical/copies/{}", p.copy_id);
        let req = UpdateCopyNoteRequest { note: p.note };
        Ok(Json(
            self.client
                .write_json(Method::PATCH, &path, &req)
                .await
                .map_err(write_error)?,
        ))
    }

    #[tool(
        description = "Permanently delete one physical copy record (\"I sold it\") — every user stops seeing it, and there is no undo. Requires the `can_edit` permission, and refuses without confirm=true: show the user the copy (list_physical_copies) and get their approval first."
    )]
    pub async fn remove_physical_copy(
        &self,
        Parameters(p): Parameters<RemoveCopyParams>,
    ) -> Result<Json<Ack>, ErrorData> {
        if !p.confirm {
            return Err(ErrorData::invalid_params(
                "confirm must be true: deleting a physical copy is permanent and library-wide. \
                 Show the user the copy (list_physical_copies) and pass confirm=true once they \
                 approve.",
                None,
            ));
        }
        let path = format!("/api/physical/copies/{}", p.copy_id);
        self.client
            .write_no_content::<()>(Method::DELETE, &path, None)
            .await
            .map_err(write_error)?;
        Ok(Json(Ack {
            message: format!("deleted physical copy {}", p.copy_id),
        }))
    }
}
