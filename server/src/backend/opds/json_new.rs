//! `GET /opds/v2/new` — OPDS 2.0 JSON feed of the most recently added books
//! (AC2). Same query, same row cap (`new::NEW_LIMIT`), and the same
//! ebook-only scope as `/opds/new`, so the two "recently added" feeds
//! always agree.

use axum::{extract::State, response::Response};
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};

use super::json_entries::book_publication;
use super::json_response;
use super::new::load_new_arrivals;
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

pub(super) async fn new_arrivals(user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let books = match load_new_arrivals(&state, &user).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Recently Added".to_string(),
            number_of_items: Some(books.len() as i64),
            ..Default::default()
        },
        links: vec![
            Link::new("/opds/v2/new")
                .with_rel("self")
                .with_type(MEDIA_TYPE),
            Link::new("/opds/v2")
                .with_rel("start")
                .with_type(MEDIA_TYPE),
        ],
        navigation: Vec::new(),
        publications: books.iter().map(book_publication).collect(),
    };
    json_response(&feed)
}
