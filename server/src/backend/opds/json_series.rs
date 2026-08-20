//! Series browse (AC2) — the JSON catalog's series parity, which the Atom
//! feed doesn't implement (see the `opds` module doc): `/opds/v2/series`
//! (every series with ≥1 visible book) and `/opds/v2/series/{id}` (one
//! series' publication feed). Reuses `db::list_series` / `db::get_series` —
//! the same reads the web series index and detail pages use — so this
//! browse can't drift from what a user sees browsing the app itself.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};

use super::json_entries::book_publication;
use super::series::load_series;
use super::{internal, json_nav_link, json_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

/// `GET /opds/v2/series` — navigation feed of every series with at least
/// one visible book, each linking to `/opds/v2/series/{id}`.
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
    let navigation = series
        .iter()
        .map(|s| json_nav_link(&s.name, &format!("/opds/v2/series/{}", s.id)))
        .collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Series".to_string(),
            ..Default::default()
        },
        links: vec![
            Link::new("/opds/v2/series")
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

/// `GET /opds/v2/series/{id}` — publication feed of one series' books, in
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
    let publications: Vec<_> = series.books.iter().map(book_publication).collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: series.name.clone(),
            number_of_items: Some(publications.len() as i64),
            ..Default::default()
        },
        links: vec![
            Link::new(format!("/opds/v2/series/{id}"))
                .with_rel("self")
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
