//! Discovery-detail reads: a single author or series with their books,
//! plus the global tag cloud. Membership and ordering follow the merged
//! (override-aware) view via the BOOK_COLUMNS template shared with the
//! book read path. Single-tenant today — every read returns all matching
//! rows without per-user ACL filtering.

pub mod authors;
pub mod series;
pub mod tags;

#[cfg(test)]
mod tests;

pub use authors::{get_author, MAX_DISCOVERY_BOOKS};
pub use series::get_series;
pub use tags::get_tag_cloud;

/// Errors returned by the discovery-detail reads.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
