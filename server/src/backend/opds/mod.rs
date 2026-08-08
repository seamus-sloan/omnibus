//! `/opds/*` — an OPDS 1.2 Atom catalog for third-party e-reader clients
//! (KOReader and similar), gated behind the same live-session auth as the
//! rest of `/api/*` (every handler here takes [`crate::auth::AuthUser`], so
//! an unauthenticated request 401s exactly like an `/api/*` one — AC4).
//! Root navigation (`/opds`), an OpenSearch description (`/opds/osd`) and
//! search (`/opds/search`), a recently-added feed (`/opds/new`), and a
//! letter-indexed author browse (`/opds/authors[/{letter}]`,
//! `/opds/author/{id}`) with per-author acquisition feeds carrying
//! download and cover links (AC1-AC4). Series and category browses are not
//! yet implemented — see #930 for the deferred scope.
//!
//! Deliberately **ebook-library scoped**: every read here filters to
//! `settings.ebook_library_path` rather than the combined ebook+audiobook
//! set `/api/ebooks` uses, so every acquisition entry this catalog emits
//! carries a working download link (`entries::download_link`'s audiobook
//! arms exist only for a book that got there via a cross-library merge).
//!
//! Mounted outside `rest_router` (own top-level router merged in
//! `main.rs`, mirroring `kobo_router`) since its routes live at `/opds/*`,
//! not `/api/*`.

use axum::{
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};

use crate::http_errors::internal;

use super::AppState;

mod atom;
mod authors;
mod entries;
mod nav;
mod new;
mod search;
#[cfg(test)]
mod tests;

/// Build the `/opds/*` router. `Extension(pool)` is layered here so the
/// router is self-contained for integration tests, mirroring `kobo_router`
/// in `super::kobo`; the live server layers the same one at the top
/// (harmless overlap, mirroring `rest_router`).
pub fn opds_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/opds", get(nav::root))
        .route("/opds/osd", get(nav::osd))
        .route("/opds/search", get(search::search))
        .route("/opds/new", get(new::new_arrivals))
        .route("/opds/authors", get(authors::letter_index))
        .route("/opds/authors/{letter}", get(authors::by_letter))
        .route("/opds/author/{id}", get(authors::acquisition_feed))
        .with_state(state)
        .layer(Extension(pool))
}

/// Wrap an already-serialized XML document as a `200` response with the
/// given OPDS/OpenSearch content type.
fn xml_response(content_type: &'static str, body: String) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// Current instant as RFC 3339, for feed-level `<updated>` where no
/// finer-grained per-row timestamp exists (nav feeds, letter indexes,
/// search results).
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Best-effort RFC 3339 conversion of a SQLite `datetime('now')`-shaped
/// timestamp (`YYYY-MM-DD HH:MM:SS`, UTC, space separator) — the shape
/// `EbookMetadata::added_at` carries. Falls back to the current instant
/// when the value is missing or doesn't parse, so a malformed or absent
/// timestamp never breaks Atom's mandatory `<updated>` element.
fn entry_updated(added_at: Option<&str>) -> String {
    let parsed = added_at.and_then(|s| {
        let format =
            time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
                .ok()?;
        time::PrimitiveDateTime::parse(s, &format).ok()
    });
    parsed
        .map(time::PrimitiveDateTime::assume_utc)
        .unwrap_or_else(time::OffsetDateTime::now_utc)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// A navigation `<entry>` — a folder-like link into another feed, carrying
/// a human-readable summary as its `<summary>`. `kind` is the *target*
/// feed's OPDS content type ([`atom::NAVIGATION_TYPE`] or
/// [`atom::ACQUISITION_TYPE`]) so the client knows what following the link
/// yields before it does.
fn nav_entry(
    id: &str,
    title: &str,
    updated: &str,
    href: &str,
    kind: &'static str,
    summary: &str,
) -> atom::Entry {
    atom::Entry {
        id: id.to_string(),
        title: title.to_string(),
        updated: updated.to_string(),
        summary: Some(summary.to_string()),
        authors: Vec::new(),
        links: vec![atom::Link::new("subsection", href.to_string(), kind)],
    }
}
