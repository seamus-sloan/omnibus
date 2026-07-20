//! Physical Check-In wire types shared between web (`#[server]`) and mobile
//! (`reqwest`): a library-wide physical copy of a book and a per-user physical
//! wishlist entry. Both reference a book by its durable `books.uuid`.

use serde::{Deserialize, Serialize};

/// A physical copy of a book, owned library-wide (shared by all users like a
/// digital file). A book can have many; each is individually deletable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalCopy {
    pub id: i64,
    pub book_uuid: String,
    /// Scanned ISBN for this copy; `None` for a manual (non-scan) add.
    pub isbn: Option<String>,
    /// User who checked the copy in; `None` if that account was later deleted.
    pub added_by_user_id: Option<i64>,
    pub checked_in_at: i64,
    /// Free-text edition/publisher note until an edition-metadata provider exists.
    pub note: Option<String>,
}

/// Where a wishlist entry was added from. Persisted as the `source` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WishlistSource {
    /// Added while resolving a barcode/ISBN scan.
    Scan,
    /// Added from a book's detail page.
    Detail,
    /// Added by hand (manual metadata entry).
    Manual,
}

impl WishlistSource {
    /// The stored `source` string (matches the migration's CHECK constraint).
    pub fn as_str(self) -> &'static str {
        match self {
            WishlistSource::Scan => "scan",
            WishlistSource::Detail => "detail",
            WishlistSource::Manual => "manual",
        }
    }

    /// Parse a stored `source` string; `None` for an unrecognized value.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "scan" => Some(WishlistSource::Scan),
            "detail" => Some(WishlistSource::Detail),
            "manual" => Some(WishlistSource::Manual),
            _ => None,
        }
    }
}

/// A book on one user's physical wishlist. Unique per `(user_id, book_uuid)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WishlistEntry {
    pub id: i64,
    pub user_id: i64,
    pub book_uuid: String,
    pub added_at: i64,
    pub source: WishlistSource,
}
