//! `GET /opds/v2/search?q=` — OPDS 2.0 JSON feed of FTS matches (AC1/AC2),
//! reached via the templated `search` link on `/opds/v2`. Reuses
//! `search::SearchQuery` so both catalogs' search endpoints accept the
//! exact same `?q=` shape.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};
use omnibus_shared::search_query_too_long;

use super::entries::retain_ereader_books;
use super::json_entries::book_publication;
use super::search::SearchQuery;
use super::{internal, json_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

pub(super) async fn search(
    _user: OpdsAuthUser,
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
    let mut books = if paths.is_empty() || params.q.trim().is_empty() {
        Vec::new()
    } else {
        match db::search_books_for_paths(&state.pool, &paths, &params.q).await {
            Ok(b) => b,
            Err(e) => return internal("search books", e),
        }
    };
    retain_ereader_books(&mut books);
    let self_href = format!("/opds/v2/search?q={}", urlencoding::encode(&params.q));
    let feed = Feed {
        metadata: FeedMetadata {
            title: format!("Search: {}", params.q),
            number_of_items: Some(books.len() as i64),
            ..Default::default()
        },
        links: vec![
            Link::new(self_href).with_rel("self").with_type(MEDIA_TYPE),
            Link::new("/opds/v2")
                .with_rel("start")
                .with_type(MEDIA_TYPE),
        ],
        navigation: Vec::new(),
        publications: books.iter().map(book_publication).collect(),
    };
    json_response(&feed)
}
