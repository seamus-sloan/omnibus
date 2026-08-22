//! `GET /opds/v2/search?q=` — OPDS 2.0 JSON feed of FTS matches (AC1/AC2),
//! reached via the templated `search` link on `/opds/v2`. Reuses
//! `search::SearchQuery` so both catalogs' search endpoints accept the
//! exact same `?q=` shape.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};

use super::json_entries::book_publication;
use super::json_response;
use super::search::{load_matches, SearchQuery};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

pub(super) async fn search(
    user: OpdsAuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let books = match load_matches(&state, &user, &params.q).await {
        Ok(Some(b)) => b,
        Ok(None) => return (StatusCode::BAD_REQUEST, "query too long").into_response(),
        Err(resp) => return resp,
    };
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
