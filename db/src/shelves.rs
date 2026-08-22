//! Shelves data layer: CRUD for smart and manual library shelves, the
//! smart-rule to SQL membership translation, and visibility-scoped listing.
//! Membership is uuid-soft-referenced (`shelf_books.book_uuid`) so a reindex or
//! scan-root repoint keeps hand-picked shelves intact; smart membership is
//! computed on read from the `shelf_rules` conditions.

use crate::books::BooksError;

pub(crate) mod provision;
mod read;
mod rules;
mod write;

#[cfg(test)]
mod tests;

pub use provision::{provision_wishlist_shelf, provision_wishlist_shelves};
pub use read::{
    get_shelf, kobo_synced_book_uuids, list_visible_shelves, manual_shelves_containing,
    preview_rule, shelf_exclusive_hidden_uuids, shelf_page, LIST_SHELVES_LIMIT,
};
pub use write::{add_books, create_shelf, delete_shelf, remove_book, update_shelf};

/// Errors from the shelves data layer.
#[derive(Debug, thiserror::Error)]
pub enum ShelfError {
    #[error("shelf not found")]
    NotFound,
    /// The owner already has a shelf with that name (case-insensitive).
    #[error("a shelf with that name already exists")]
    NameTaken,
    #[error("book not found")]
    BookNotFound,
    /// A smart rule couldn't be translated (bad value, unsupported field/op).
    #[error("invalid rule: {0}")]
    InvalidRule(String),
    /// The target is a system shelf (e.g. the built-in Wishlist): it can't be
    /// renamed, deleted, reconfigured, or have its membership edited by hand.
    #[error("system shelves cannot be modified")]
    SystemShelf,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<BooksError> for ShelfError {
    fn from(e: BooksError) -> Self {
        match e {
            BooksError::Db(inner) => Self::Sqlx(inner),
            // The only `BooksError`-returning call here is the uuid resolver,
            // which never reads overrides — fold defensively.
            BooksError::OverridesJson(inner) => Self::Sqlx(sqlx::Error::Decode(Box::new(inner))),
            BooksError::Other(msg) => Self::Sqlx(sqlx::Error::Decode(msg.into())),
        }
    }
}

/// Whether `viewer` may see `shelf`: owner, an admin, or a public shelf.
pub fn can_view(shelf: &omnibus_shared::Shelf, viewer_id: i64, is_admin: bool) -> bool {
    shelf.owner_user_id == viewer_id
        || is_admin
        || shelf.visibility == omnibus_shared::Visibility::Public
}

/// Whether `viewer` may mutate `shelf`: owner or admin only.
pub fn can_edit(shelf: &omnibus_shared::Shelf, viewer_id: i64, is_admin: bool) -> bool {
    shelf.owner_user_id == viewer_id || is_admin
}
