//! `GET /opds/all` — paged acquisition feed of the entire (ebook-scoped)
//! library in title order, with `rel="next"` keyset pagination so a client
//! can walk the whole collection without one unbounded Atom document.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db::{self as db, CursorError, PageCursor};
use omnibus_shared::{SortDir, SortKey, ViewFilters};

use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::{book_entry, retain_ereader_books};
use super::{internal, now_rfc3339, xml_response};

/// Page size for the All Books feed. `pub(super)` — `opds::json_all` uses
/// the same cap so the two catalogs' pages can't drift apart in size.
/// A page may carry fewer entries than this after the e-reader-format
/// filter runs; the cursor still advances over the unfiltered rows, so no
/// book is ever skipped or repeated.
pub(super) const ALL_LIMIT: i64 = 50;

#[derive(serde::Deserialize)]
pub(super) struct AllQuery {
    pub cursor: Option<String>,
}

/// Decode a client-supplied `?cursor=`. Typed error rather than a ready
/// 400 so the `Err` variant stays small (clippy `result_large_err`);
/// callers map it through [`bad_cursor_response`]. `pub(super)` — shared
/// with `opds::json_all`.
pub(super) fn parse_cursor(raw: Option<&str>) -> Result<Option<PageCursor>, CursorError> {
    raw.map(PageCursor::decode).transpose()
}

/// The 400 a malformed cursor maps to, in one place so both catalogs
/// answer identically rather than silently restarting from page one.
pub(super) fn bad_cursor_response() -> Response {
    (StatusCode::BAD_REQUEST, "malformed pagination cursor").into_response()
}

/// Fetch one page of the ebook-scoped library for the All Books feeds:
/// title order, e-reader formats only. `pub(super)` — the single read both
/// catalogs' All Books pages come from.
pub(super) async fn load_page(
    state: &AppState,
    cursor: Option<&PageCursor>,
) -> Result<db::BookPage, Response> {
    let settings = db::get_settings(&state.pool)
        .await
        .map_err(|e| internal("read settings", e))?;
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    if paths.is_empty() {
        return Ok(db::BookPage {
            books: Vec::new(),
            next: None,
        });
    }
    let mut page = db::list_books_page(
        &state.pool,
        &paths,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        cursor,
        ALL_LIMIT,
    )
    .await
    .map_err(|e| internal("list all books", e))?;
    retain_ereader_books(&mut page.books);
    Ok(page)
}

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
        Link::new(
            "self",
            self_href("/opds/all", query.cursor.as_deref()),
            ACQUISITION_TYPE,
        ),
        Link::new("start", "/opds", NAVIGATION_TYPE),
    ];
    if let Some(next) = &page.next {
        links.push(Link::new(
            "next",
            format!("/opds/all?cursor={}", next.encode()),
            ACQUISITION_TYPE,
        ));
    }
    let feed = Feed {
        id: "urn:omnibus:opds:all".to_string(),
        title: "All Books".to_string(),
        updated: now_rfc3339(),
        links,
        entries: page.books.iter().map(book_entry).collect(),
    };
    xml_response(ACQUISITION_TYPE, feed.to_xml())
}

/// The page's own href, cursor included — a `rel="self"` that drops the
/// cursor would claim every page is page one. `pub(super)` — shared with
/// `opds::json_all` (which passes its `/opds/v2/all` base).
pub(super) fn self_href(base: &str, cursor: Option<&str>) -> String {
    match cursor {
        Some(c) => format!("{base}?cursor={}", urlencoding::encode(c)),
        None => base.to_string(),
    }
}
