//! `GET /opds/search` — OPDS acquisition feed of FTS matches (AC3), reached
//! via the `Url` template in `/opds/osd`.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::search_query_too_long;
use serde::Deserialize;

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::book_entry;
use super::{internal, now_rfc3339, xml_response};
use crate::auth::AuthUser;
use crate::backend::AppState;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    #[serde(default)]
    q: String,
}

pub(super) async fn search(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    if search_query_too_long(&params.q) {
        return (StatusCode::BAD_REQUEST, "query too long").into_response();
    }
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    // OPDS scope is the ebook library only (see `opds` module doc) — an
    // audiobook-only match would carry no working acquisition link.
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    let books = if paths.is_empty() || params.q.trim().is_empty() {
        Vec::new()
    } else {
        match db::search_books_for_paths(&state.pool, &paths, &params.q).await {
            Ok(b) => b,
            Err(e) => return internal("search books", e),
        }
    };
    let feed = Feed {
        id: "urn:omnibus:opds:search".to_string(),
        title: format!("Search: {}", params.q),
        updated: now_rfc3339(),
        links: vec![
            Link::new("self", "/opds/search", ACQUISITION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries: books.iter().map(book_entry).collect(),
    };
    xml_response(ACQUISITION_TYPE, feed.to_xml())
}
