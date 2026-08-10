//! `GET /opds/v2/shelves` + `/opds/v2/shelves/{id}` — the OPDS 2.0 JSON
//! counterpart to the Atom shelves browse. Shares `shelves::load_visible_shelf`
//! (and through it the `db::can_view` gate) so the two catalogs can't
//! drift on who may see which shelf.

use axum::{
    extract::{Path, State},
    response::Response,
};
use omnibus_db as db;
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};
use omnibus_shared::{SortDir, SortKey};

use super::entries::retain_ereader_books;
use super::json_entries::book_publication;
use super::shelves::load_visible_shelf;
use super::{internal, json_nav_link, json_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

pub(super) async fn index(user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let shelves = match db::list_visible_shelves(&state.pool, user.0.id, user.0.is_admin).await {
        Ok(s) => s,
        Err(e) => return internal("list shelves", e),
    };
    let navigation = shelves
        .iter()
        .map(|s| json_nav_link(&s.name, &format!("/opds/v2/shelves/{}", s.id)))
        .collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Shelves".to_string(),
            ..Default::default()
        },
        links: vec![
            Link::new("/opds/v2/shelves")
                .with_rel("self")
                .with_type(MEDIA_TYPE),
            Link::new("/opds/v2")
                .with_rel("start")
                .with_type(MEDIA_TYPE),
        ],
        navigation,
        publications: Vec::new(),
    };
    json_response(&feed)
}

pub(super) async fn acquisition_feed(
    user: OpdsAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    let shelf = match load_visible_shelf(&state, &user, id).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let mut page = match db::shelf_page(&state.pool, &shelf, SortKey::Title, SortDir::Asc).await {
        Ok(p) => p,
        Err(e) => return internal("read shelf members", e),
    };
    // Same non-path-scoped read + subsuming format filter as the Atom
    // feed — see `shelves::acquisition_feed` for why this satisfies the
    // module's ebook-library invariant.
    retain_ereader_books(&mut page.books);
    let publications: Vec<_> = page.books.iter().map(book_publication).collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: shelf.name.clone(),
            number_of_items: Some(publications.len() as i64),
            ..Default::default()
        },
        links: vec![
            Link::new(format!("/opds/v2/shelves/{id}"))
                .with_rel("self")
                .with_type(MEDIA_TYPE),
            Link::new("/opds/v2/shelves")
                .with_rel("up")
                .with_type(MEDIA_TYPE),
            Link::new("/opds/v2")
                .with_rel("start")
                .with_type(MEDIA_TYPE),
        ],
        navigation: Vec::new(),
        publications,
    };
    json_response(&feed)
}
