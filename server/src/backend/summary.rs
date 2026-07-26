//! On-demand book-summary fetch REST handlers (mobile-facing); web hits the
//! analogous `/api/rpc/ebook/summary/*` server functions in
//! `omnibus_frontend::rpc::summary`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::summary::SummarySource;
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Body for `POST /api/ebooks/{uuid}/summary/fetch`.
#[derive(Debug, Deserialize)]
pub(super) struct FetchSummaryBody {
    source: SummarySource,
}

/// Fetch a summary for a book from `source`. Requires `can_edit` or admin.
/// Returns the fetched text, or `null` when that source has no summary — a
/// clean miss the client cascades past. External network failures surface as
/// an opaque 500.
pub(super) async fn post_ebook_summary_fetch(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(body): Json<FetchSummaryBody>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    match db::fetch_summary(&state.pool, &uuid, body.source).await {
        Ok(text) => Json(text).into_response(),
        Err(e) => internal("fetch summary", e),
    }
}

/// The ordered summary-source cascade for the current settings (Hardcover when
/// keyed → Google Books always → Open Library only when no keys). Any
/// authenticated user may read this non-sensitive plan; it drives which sources
/// the client attempts, and in what order.
pub(super) async fn get_summary_sources(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Response {
    match db::summary_source_plan(&state.pool).await {
        Ok(sources) => Json(sources).into_response(),
        Err(e) => internal("summary source plan", e),
    }
}

#[cfg(test)]
mod tests;
