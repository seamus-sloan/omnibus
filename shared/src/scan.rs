//! Wire types for the Physical Check-In scan flow: the resolution outcome a
//! scanned/typed ISBN maps to, and the write-request bodies for each branch of
//! the decision tree (check in, add a physical-only book, wishlist).

use serde::{Deserialize, Serialize};

use crate::metadata_lookup::ExternalBookMeta;
use crate::physical::WishlistSource;

/// A library book matched during scan resolution — just enough to display and
/// act on it (the confirm/decision screens).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanBook {
    pub uuid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub cover_url: Option<String>,
    /// Whether the book already has ≥1 physical copy checked in.
    pub has_physical: bool,
    /// The book's own ISBN identifier, separator-stripped. Lets the close-match
    /// confirm show the library edition's ISBN beside the scanned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
}

/// Outcome of resolving a scanned/typed ISBN down the matching ladder. A fuzzy
/// (title, author) hit is a [`ScanOutcome::CloseMatch`] — never auto-resolved;
/// the client must confirm before any write, so a hit on several rows is a
/// picker rather than a reason to decline the match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanOutcome {
    /// Exact identifier hit; the book already has a physical copy.
    AlreadyOwned { book: ScanBook },
    /// Exact identifier hit; on the caller's physical wishlist (no physical
    /// copy yet). Routes to the book's detail page, like [`Self::AlreadyOwned`]
    /// — the reader already tracks this one, so don't offer to check it in blind.
    OnWishlist { book: ScanBook },
    /// Exact identifier hit; in the library digitally, no physical copy yet.
    InLibraryUnowned { book: ScanBook },
    /// Online-resolved and (title, author)-matched at least one library book —
    /// needs a human "is this the book?" confirmation before any write.
    ///
    /// Carried as head + tail rather than one list so the single-candidate wire
    /// shape is byte-identical to what shipped: a client that only knows `book`
    /// still decodes the outcome and renders a usable confirm screen instead of
    /// failing on an unknown shape. Clients that know both render `book`
    /// followed by `others` as one picker.
    CloseMatch {
        book: ScanBook,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        others: Vec<ScanBook>,
        scanned: ExternalBookMeta,
    },
    /// Online-resolved but not in the library.
    NotInLibrary { online: ExternalBookMeta },
    /// Neither the library nor any provider knew the ISBN.
    Unresolved,
}

/// Resolve a scanned/typed ISBN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveRequest {
    pub isbn: String,
}

impl ResolveRequest {
    /// Reject an empty/oversized `isbn`, mirroring the same cap applied to
    /// [`CheckInRequest::isbn`]. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.isbn.trim().is_empty() {
            return Err("isbn is required".into());
        }
        if self.isbn.chars().count() > ISBN_MAX_LEN {
            return Err(format!("isbn exceeds {ISBN_MAX_LEN} characters"));
        }
        Ok(())
    }
}

/// Maximum length (in chars) for a free-text physical check-in `note` field.
/// Mirrors `omnibus_shared::highlight::UpdateHighlightNote::NOTE_MAX_LEN`.
pub const NOTE_MAX_LEN: usize = 4096;

/// Maximum length (in chars) for a title-search `query` — generous for any
/// real book title while still bounding the provider request.
pub const SEARCH_QUERY_MAX_LEN: usize = 200;

/// Search the external providers by title text — the fallback offered when an
/// ISBN scan resolves to [`ScanOutcome::Unresolved`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanSearchRequest {
    pub query: String,
}

impl ScanSearchRequest {
    /// Reject a blank or oversized `query`. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.query.trim().is_empty() {
            return Err("query is required".into());
        }
        if self.query.chars().count() > SEARCH_QUERY_MAX_LEN {
            return Err(format!("query exceeds {SEARCH_QUERY_MAX_LEN} characters"));
        }
        Ok(())
    }
}

/// Title-search results: provider-resolved candidates for the reader to pick
/// from, each complete enough to feed [`ResolveMetaRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanSearchResponse {
    pub results: Vec<ExternalBookMeta>,
}

/// Resolve a picked title-search candidate against the library — the ladder's
/// library rungs applied to metadata already in hand, so no provider round
/// trip (which could miss on an ISBN the search itself just surfaced).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveMetaRequest {
    pub meta: ExternalBookMeta,
}

impl ResolveMetaRequest {
    /// Delegates field-length checks to [`ExternalBookMeta::validate`].
    /// Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        self.meta.validate()
    }
}

/// Maximum length (in chars) for a scanned/typed `isbn` string on
/// [`CheckInRequest`], measured before separators are stripped — generous
/// headroom over the 13-digit canonical form.
pub const ISBN_MAX_LEN: usize = 32;

/// Validate an optional free-text note against [`NOTE_MAX_LEN`]. Shared by
/// every request type in this module that carries a `note` field.
fn validate_note(note: &Option<String>) -> Result<(), String> {
    if let Some(v) = note {
        if v.chars().count() > NOTE_MAX_LEN {
            return Err(format!("note exceeds {NOTE_MAX_LEN} characters"));
        }
    }
    Ok(())
}

/// Check in a physical copy of a book already in the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckInRequest {
    pub book_uuid: String,
    #[serde(default)]
    pub isbn: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl CheckInRequest {
    /// Reject an empty/oversized `book_uuid`, an oversized `isbn`, or an
    /// oversized `note`. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if self.book_uuid.len() > crate::BOOK_UUID_MAX_LEN {
            return Err(format!(
                "book uuid must be ≤ {} bytes",
                crate::BOOK_UUID_MAX_LEN
            ));
        }
        if let Some(ref isbn) = self.isbn {
            if isbn.chars().count() > ISBN_MAX_LEN {
                return Err(format!("isbn exceeds {ISBN_MAX_LEN} characters"));
            }
        }
        validate_note(&self.note)
    }
}

/// Add a physical-only book (not in the library) from resolved external meta —
/// creates a fileless book plus its first physical copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddPhysicalOnlyRequest {
    pub meta: ExternalBookMeta,
    #[serde(default)]
    pub note: Option<String>,
}

impl AddPhysicalOnlyRequest {
    /// Delegates field-length checks to [`ExternalBookMeta::validate`], plus
    /// the request's own `note` cap. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        self.meta.validate()?;
        validate_note(&self.note)
    }
}

/// Add a book to the caller's physical wishlist — either an existing library
/// book (`book_uuid`) or a new fileless book from external meta. Set one; if
/// both are present `book_uuid` wins and `meta` is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WishlistAddRequest {
    #[serde(default)]
    pub book_uuid: Option<String>,
    #[serde(default)]
    pub meta: Option<ExternalBookMeta>,
    pub source: WishlistSource,
}

impl WishlistAddRequest {
    /// Reject an oversized `book_uuid` or a `meta` that fails its own
    /// validation. `meta` is only validated when `book_uuid` is absent —
    /// mirroring the "book_uuid wins, meta is ignored" precedence documented
    /// on the struct — so a valid `book_uuid` alongside a junk `meta` isn't
    /// spuriously rejected for a field that would never be used. Handlers
    /// translate `Err(_)` into 400. Whether at least one of `book_uuid`/`meta`
    /// is set is a domain rule enforced downstream
    /// (`ScanError::MissingWishlistTarget`), not a shape check here.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref uuid) = self.book_uuid {
            if uuid.trim().is_empty() {
                return Err("book_uuid is required".into());
            }
            if uuid.len() > crate::BOOK_UUID_MAX_LEN {
                return Err(format!(
                    "book uuid must be ≤ {} bytes",
                    crate::BOOK_UUID_MAX_LEN
                ));
            }
        } else if let Some(ref meta) = self.meta {
            meta.validate()?;
        }
        Ok(())
    }
}

/// The uuid of the book a check-in / add / wishlist write landed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookRef {
    pub book_uuid: String,
}

#[cfg(test)]
mod tests;
