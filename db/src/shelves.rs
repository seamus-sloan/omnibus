//! Shelves data layer: CRUD for smart/manual library shelves, the smart-rule
//! → SQL membership translation, and visibility-scoped listing.
//!
//! Membership is uuid-soft-referenced (`shelf_books.book_uuid`) so a reindex or
//! scan-root repoint keeps hand-picked shelves intact. Smart membership is
//! computed on read from the `shelf_rules` conditions; owner-scoped fields
//! (rating) resolve against the shelf's `owner_user_id`.

use crate::books::BooksError;

mod read;
mod rules;
mod write;

#[cfg(test)]
mod tests;

pub use read::{get_shelf, list_visible_shelves, preview_rule, shelf_page};
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
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<BooksError> for ShelfError {
    fn from(e: BooksError) -> Self {
        match e {
            BooksError::Db(inner) => Self::Sqlx(inner),
            // The only `BooksError`-returning call here is the uuid resolver,
            // which never decodes overrides JSON — fold defensively.
            BooksError::OverridesJson(inner) => Self::Sqlx(sqlx::Error::Decode(Box::new(inner))),
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
