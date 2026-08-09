//! Discovery-detail reads: a single author or series with their books,
//! plus the global tag and genre clouds. Membership and ordering follow
//! the merged (override-aware) view via the BOOK_COLUMNS template shared
//! with the book read path. Single-tenant today — every read returns all
//! matching rows without per-user ACL filtering.

// Submodules are private — `db/src/lib.rs` does `pub use discovery::*`,
// so any `pub mod` here would expose `omnibus_db::authors`, etc. to
// downstream crates, which is a new public path that didn't exist
// before the split. Matches the leaf-submodule-private precedent in
// `db/src/books.rs`; only the named items are re-exported below.
mod authors;
mod genres;
mod series;
mod tags;

#[cfg(test)]
mod tests;

pub use authors::{get_author, get_author_for_paths, MAX_DISCOVERY_BOOKS};
pub use genres::get_genre_cloud;
pub use series::get_series;
pub use tags::get_tag_cloud;

/// Errors returned by the discovery-detail reads.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
