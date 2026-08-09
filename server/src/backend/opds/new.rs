//! `GET /opds/new` — OPDS acquisition feed of the most recently added books.

use axum::{extract::State, response::Response};
use omnibus_db as db;
use omnibus_shared::{SortDir, SortKey, ViewFilters};

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::book_entry;
use super::{internal, now_rfc3339, xml_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

/// Row cap for the recently-added feed — generous for a client's first
/// screen without needing to page further. This endpoint doesn't expose a
/// `?cursor=`; a client that wants more follows `/opds/authors` instead.
/// `pub(super)` — `opds::json_new` reuses the same cap so both catalogs'
/// "recently added" feeds can't drift apart in size.
pub(super) const NEW_LIMIT: i64 = 50;

pub(super) async fn new_arrivals(_user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    // OPDS scope is the ebook library only — see the `opds` module doc.
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    let books = if paths.is_empty() {
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
    let feed = Feed {
        id: "urn:omnibus:opds:new".to_string(),
        title: "Recently Added".to_string(),
        updated: now_rfc3339(),
        links: vec![
            Link::new("self", "/opds/new", ACQUISITION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries: books.iter().map(book_entry).collect(),
    };
    xml_response(ACQUISITION_TYPE, feed.to_xml())
}
