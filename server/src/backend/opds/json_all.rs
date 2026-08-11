//! `GET /opds/v2/all` — the OPDS 2.0 JSON counterpart to `/opds/all`:
//! same read, same page size, same cursor scheme (`all::load_page` /
//! `all::parse_cursor`), so the two catalogs page identically.

use axum::{
    extract::{Query, State},
    response::Response,
};
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};

use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

use super::all::{bad_cursor_response, load_page, parse_cursor, self_href, AllQuery};
use super::json_entries::book_publication;
use super::json_response;

pub(super) async fn all_books(
    _user: OpdsAuthUser,
    State(state): State<AppState>,
    Query(query): Query<AllQuery>,
) -> Response {
    let cursor = match parse_cursor(query.cursor.as_deref()) {
        Ok(c) => c,
        Err(_) => return bad_cursor_response(),
    };
    let page = match load_page(&state, cursor.as_ref()).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let mut links = vec![
        Link::new(self_href("/opds/v2/all", query.cursor.as_deref()))
            .with_rel("self")
            .with_type(MEDIA_TYPE),
        Link::new("/opds/v2")
            .with_rel("start")
            .with_type(MEDIA_TYPE),
    ];
    if let Some(next) = &page.next {
        links.push(
            Link::new(format!("/opds/v2/all?cursor={}", next.encode()))
                .with_rel("next")
                .with_type(MEDIA_TYPE),
        );
    }
    let publications: Vec<_> = page.books.iter().map(book_publication).collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: "All Books".to_string(),
            number_of_items: Some(publications.len() as i64),
            ..Default::default()
        },
        links,
        navigation: Vec::new(),
        publications,
    };
    json_response(&feed)
}
