//! `GET /opds/v2/new` — OPDS 2.0 JSON feed of the most recently added books
//! (AC2). Same query, same row cap (`new::NEW_LIMIT`), and the same
//! ebook-only scope as `/opds/new`, so the two "recently added" feeds
//! always agree.

use axum::{extract::State, response::Response};
use omnibus_db as db;
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};
use omnibus_shared::{SortDir, SortKey, ViewFilters};

use super::entries::retain_ereader_books;
use super::json_entries::book_publication;
use super::new::NEW_LIMIT;
use super::{internal, json_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

pub(super) async fn new_arrivals(_user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    // OPDS scope is the ebook library only — see the `opds` module doc.
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    let mut books = if paths.is_empty() {
        Vec::new()
    } else {
        match db::list_books_page(
            &state.pool,
            &paths,
            SortKey::NewestAdded,
            SortDir::Desc,
            &ViewFilters::default(),
            &[],
            None,
            NEW_LIMIT,
        )
        .await
        {
            Ok(page) => page.books,
            Err(e) => return internal("list recently added books", e),
        }
    };
    retain_ereader_books(&mut books);
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
