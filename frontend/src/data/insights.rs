//! Per-book Started/Time-read/Sessions fetch for the book-detail page's
//! Insights card. `BdRailSection` (the only caller) doesn't compile on
//! mobile, so this is a server-function wrapper only — no REST transport.

use omnibus_shared::BookInsights;

use super::{note_server_fn_err, DataError};

/// Web/SSR: fetch a book's reading/listening insights via
/// `rpc_book_insights`. `None` when the book has no recorded sessions yet.
///
/// Declares the device's offset like every other stats read (rule 10): the
/// card's per-day activity strip and the `as_of_day` it anchors against are
/// day-bucketed, and this strip sits beside the user-wide heatmap — so a
/// session must not land on one day here and another there.
pub async fn get_book_insights(uuid: &str) -> Result<Option<BookInsights>, DataError> {
    crate::rpc::rpc_book_insights(uuid.to_string(), crate::time::local_utc_offset_minutes())
        .await
        .map_err(note_server_fn_err)
}
