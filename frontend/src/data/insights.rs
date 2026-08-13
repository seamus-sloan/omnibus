//! Per-book Started/Time-read/Sessions fetch for the book-detail page's
//! Insights card. `BdRailSection` (the only caller) doesn't compile on
//! mobile, so this is a server-function wrapper only — no REST transport.

use omnibus_shared::BookInsights;

use super::{note_server_fn_err, DataError};

/// Web/SSR: fetch a book's reading/listening insights via
/// `rpc_book_insights`. `None` when the book has no recorded sessions yet.
pub async fn get_book_insights(uuid: &str) -> Result<Option<BookInsights>, DataError> {
    crate::rpc::rpc_book_insights(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
