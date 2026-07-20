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
}

/// Outcome of resolving a scanned/typed ISBN down the matching ladder. A fuzzy
/// (title, author) hit is a [`ScanOutcome::CloseMatch`] — never auto-resolved;
/// the client must confirm before any write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanOutcome {
    /// Exact identifier hit; the book already has a physical copy.
    AlreadyOwned { book: ScanBook },
    /// Exact identifier hit; in the library digitally, no physical copy yet.
    InLibraryUnowned { book: ScanBook },
    /// Online-resolved and (title, author)-matched a library book — needs a
    /// human "is this the book?" confirmation before any write.
    CloseMatch {
        book: ScanBook,
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

/// Check in a physical copy of a book already in the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckInRequest {
    pub book_uuid: String,
    #[serde(default)]
    pub isbn: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Add a physical-only book (not in the library) from resolved external meta —
/// creates a fileless book plus its first physical copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddPhysicalOnlyRequest {
    pub meta: ExternalBookMeta,
    #[serde(default)]
    pub note: Option<String>,
}

/// Add a book to the caller's physical wishlist — either an existing library
/// book (`book_uuid`) or a new fileless book from external meta. Exactly one of
/// the two must be set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WishlistAddRequest {
    #[serde(default)]
    pub book_uuid: Option<String>,
    #[serde(default)]
    pub meta: Option<ExternalBookMeta>,
    pub source: WishlistSource,
}

/// The uuid of the book a check-in / add / wishlist write landed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookRef {
    pub book_uuid: String,
}
