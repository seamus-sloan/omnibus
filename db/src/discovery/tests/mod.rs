//! Unit tests for the author/series discovery-page queries and the tag and
//! genre clouds, split by sub-topic into the sibling modules below.

mod author;
mod clouds;
mod overrides;
mod series;

use crate::physical::{create_fileless_book, FilelessBook};

/// A fileless book under the physical pseudo-root — what a wishlist add mints.
async fn seed_fileless(pool: &sqlx::SqlitePool, title: &str, author: &str) -> String {
    create_fileless_book(
        pool,
        FilelessBook {
            title: title.to_string(),
            authors: vec![author.to_string()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap()
}
