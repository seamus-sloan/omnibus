//! Shelves browse: `/opds/shelves` (navigation feed of the viewer's
//! visible shelves) and `/opds/shelves/{id}` (one shelf's acquisition
//! feed, title order). The one OPDS surface that reads the caller's
//! identity: listing via `db::list_visible_shelves`, per-shelf access
//! re-checked with `db::can_view` — a non-visible shelf 404s, never 403s.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::{Shelf, SortDir, SortKey};

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::{book_entry, retain_ereader_books};
use super::{internal, nav_entry, now_rfc3339, xml_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

/// `GET /opds/shelves` — navigation feed of the viewer's visible shelves,
/// each linking to its `/opds/shelves/{id}` acquisition feed.
pub(super) async fn index(user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let shelves = match db::list_visible_shelves(&state.pool, user.0.id, user.0.is_admin).await {
        Ok(s) => s,
        Err(e) => return internal("list shelves", e),
    };
    let updated = now_rfc3339();
    let entries = shelves
        .iter()
        .map(|s| {
            nav_entry(
                &format!("urn:omnibus:opds:shelf:{}", s.id),
                &s.name,
                &updated,
                &format!("/opds/shelves/{}", s.id),
                ACQUISITION_TYPE,
                &format!("{} book(s), by {}", s.book_count, s.owner_username),
            )
        })
        .collect();
    let feed = Feed {
        id: "urn:omnibus:opds:shelves".to_string(),
        title: "Shelves".to_string(),
        updated,
        links: vec![
            Link::new("self", "/opds/shelves", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries,
    };
    xml_response(NAVIGATION_TYPE, feed.to_xml())
}

/// Resolve `id` to a shelf the viewer may see, or the ready-to-return
/// response. Unknown id and forbidden shelf both map to 404 — a 403 would
/// confirm a private shelf's existence to a guessing client. `pub(super)`
/// — shared with `opds::json_shelves` so both catalogs gate identically.
pub(super) async fn load_visible_shelf(
    state: &AppState,
    user: &OpdsAuthUser,
    id: i64,
) -> Result<Shelf, Response> {
    let shelf = match db::get_shelf(&state.pool, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Err(StatusCode::NOT_FOUND.into_response()),
        Err(e) => return Err(internal("read shelf", e)),
    };
    if !db::can_view(&shelf, user.0.id, user.0.is_admin) {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    Ok(shelf)
}

/// `GET /opds/shelves/{id}` — acquisition feed of one visible shelf's
/// members.
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
    // `shelf_page` is not path-scoped, but the format filter subsumes the
    // module's ebook-library invariant here: EPUB/CBZ files exist only
    // under the ebook library, so members from anywhere else (e.g. the
    // audiobook library, on a mixed shelf) drop out. Pinned by
    // `shelf_feeds_exclude_non_ebook_library_members`.
    retain_ereader_books(&mut page.books);
    let feed = Feed {
        id: format!("urn:omnibus:opds:shelf:{id}"),
        title: shelf.name.clone(),
        updated: now_rfc3339(),
        links: vec![
            Link::new("self", format!("/opds/shelves/{id}"), ACQUISITION_TYPE),
            Link::new("up", "/opds/shelves", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries: page.books.iter().map(book_entry).collect(),
    };
    xml_response(ACQUISITION_TYPE, feed.to_xml())
}
