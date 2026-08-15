//! `GET /opds/search` — OPDS acquisition feed of FTS matches (AC3), reached
//! via the `Url` template in `/opds/osd`.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::{search_query_too_long, EbookMetadata};
use serde::Deserialize;

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::{book_entry, retain_ereader_books};
use super::{internal, now_rfc3339, xml_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    // `pub(super)` — `opds::json_search` reuses this exact query shape so
    // the two catalogs' search endpoints accept an identical `?q=`.
    #[serde(default)]
    pub(super) q: String,
}

/// Run the FTS match: e-reader formats only, narrowed to books `user` may
/// see (#932). `pub(super)` — the single read both catalogs' search feeds
/// come from. `Ok(None)` means the query itself was rejected (too long);
/// callers map it to the 400 both catalogs share.
pub(super) async fn load_matches(
    state: &AppState,
    user: &OpdsAuthUser,
    q: &str,
) -> Result<Option<Vec<EbookMetadata>>, Response> {
    if search_query_too_long(q) {
        return Ok(None);
    }
    let settings = db::get_settings(&state.pool)
        .await
        .map_err(|e| internal("read settings", e))?;
    // OPDS scope is the ebook library only (see `opds` module doc) — an
    // audiobook-only match would carry no working acquisition link.
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    let mut books = if paths.is_empty() || q.trim().is_empty() {
        Vec::new()
    } else {
        db::search_books_for_paths(&state.pool, &paths, q)
            .await
            .map_err(|e| internal("search books", e))?
    };
    retain_ereader_books(&mut books);
    super::retain_shelf_visible(state, user, &mut books).await?;
    Ok(Some(books))
}

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
    let self_href = format!("/opds/search?q={}", urlencoding::encode(&params.q));
    let feed = Feed {
        id: "urn:omnibus:opds:search".to_string(),
        title: format!("Search: {}", params.q),
        updated: now_rfc3339(),
        links: vec![
            Link::new("self", self_href, ACQUISITION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries: books.iter().map(book_entry).collect(),
    };
    xml_response(ACQUISITION_TYPE, feed.to_xml())
}
