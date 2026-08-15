//! Atom series browse — parity with the JSON catalog's `json_series`,
//! which shipped first: `/opds/series` (navigation feed of every series
//! with ≥1 visible book) and `/opds/series/{id}` (one series' acquisition
//! feed, in series-index order). Reuses `db::list_series` /
//! `db::get_series`, the same reads the web series pages use.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::SeriesDetail;

use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::{book_entry, retain_ereader_books};
use super::{internal, nav_entry, now_rfc3339, xml_response};

/// `GET /opds/series` — navigation feed of every series, each linking to
/// its `/opds/series/{id}` acquisition feed.
pub(super) async fn index(_user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    // OPDS scope is the ebook library only — see the `opds` module doc.
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    let series = if paths.is_empty() {
        Vec::new()
    } else {
        match db::list_series(&state.pool, &paths).await {
            Ok(s) => s,
            Err(e) => return internal("list series", e),
        }
    };
    let updated = now_rfc3339();
    let entries = series
        .iter()
        .map(|s| {
            nav_entry(
                &format!("urn:omnibus:opds:series:{}", s.id),
                &s.name,
                &updated,
                &format!("/opds/series/{}", s.id),
                ACQUISITION_TYPE,
                &format!("Books in {}", s.name),
            )
        })
        .collect();
    let feed = Feed {
        id: "urn:omnibus:opds:series".to_string(),
        title: "Series".to_string(),
        updated,
        links: vec![
            Link::new("self", "/opds/series", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries,
    };
    xml_response(NAVIGATION_TYPE, feed.to_xml())
}

/// `GET /opds/series/{id}` — acquisition feed of one series' books, in
/// series-index order (`db::get_series`'s ordering).
pub(super) async fn acquisition_feed(
    user: OpdsAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    let series = match load_series(&state, &user, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(resp) => return resp,
    };
    let feed = Feed {
        id: format!("urn:omnibus:opds:series:{id}"),
        title: series.name.clone(),
        updated: now_rfc3339(),
        links: vec![
            Link::new("self", format!("/opds/series/{id}"), ACQUISITION_TYPE),
            Link::new("up", "/opds/series", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries: series.books.iter().map(book_entry).collect(),
    };
    xml_response(ACQUISITION_TYPE, feed.to_xml())
}

/// Load one series with its books narrowed to e-reader formats and to
/// books `user` may see (#932) — an audiobook/physical-only book, or one
/// confined to a manual shelf `user` cannot see, would otherwise surface
/// as an entry with no working acquisition link. `Ok(None)` is an unknown
/// series id; `Err` is a ready-to-return failure response. `pub(super)` —
/// reused by `opds::json_series` so both catalogs' series feeds carry the
/// same entries.
pub(super) async fn load_series(
    state: &AppState,
    user: &OpdsAuthUser,
    series_id: i64,
) -> Result<Option<SeriesDetail>, Response> {
    let Some(mut series) = db::get_series(&state.pool, series_id)
        .await
        .map_err(|e| internal("read series", e))?
    else {
        return Ok(None);
    };
    retain_ereader_books(&mut series.books);
    super::retain_shelf_visible(state, user, &mut series.books).await?;
    Ok(Some(series))
}
