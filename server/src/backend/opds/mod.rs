//! `/opds/*` — an OPDS 1.2 Atom catalog for third-party e-reader clients,
//! gated behind the same live-session auth as `/api/*` (each handler takes
//! [`crate::auth::AuthUser`] directly). Deliberately ebook-library scoped —
//! every read filters to `settings.ebook_library_path` — so every
//! acquisition entry carries a working download link. The Atom catalog's
//! series and category browses are not implemented; the `/opds/v2/*` JSON
//! catalog below adds a series browse and carries category (subject/genre)
//! data inline on every publication.
//!
//! `/opds/v2/*` is the OPDS 2.0 JSON counterpart (`application/opds+json`,
//! [`omnibus_shared::opds`]) for modern OPDS clients — a distinct path
//! rather than `Accept`-negotiated on `/opds` itself, so a client (or a
//! proxy cache keyed on path) never needs content negotiation to reach it,
//! and so this module's routing table stays as simple as the Atom one's.
//! The `json_*` submodules mirror the flat `atom`-era module names
//! one-for-one (`json_nav` ~ `nav`, `json_authors` ~ `authors`, …) and
//! reuse their DB reads and format-selection helpers directly, so the two
//! catalogs cannot silently drift on what a book, author, or search result
//! actually is.

use axum::{
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use omnibus_shared::opds as opds2;

use crate::http_errors::internal;

use super::AppState;

mod atom;
mod authors;
mod entries;
mod json_authors;
mod json_entries;
mod json_nav;
mod json_new;
mod json_search;
mod json_series;
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
        .route("/opds/v2", get(json_nav::root))
        .route("/opds/v2/search", get(json_search::search))
        .route("/opds/v2/new", get(json_new::new_arrivals))
        .route("/opds/v2/authors", get(json_authors::letter_index))
        .route("/opds/v2/authors/{letter}", get(json_authors::by_letter))
        .route("/opds/v2/author/{id}", get(json_authors::acquisition_feed))
        .route("/opds/v2/series", get(json_series::index))
        .route("/opds/v2/series/{id}", get(json_series::acquisition_feed))
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

/// Serialize an [`opds2::Feed`] as a `200 application/opds+json` response —
/// the JSON analog of [`xml_response`]. Serialization only fails if a
/// `Feed` somehow carries non-UTF-8/non-finite float data it can't have
/// (every field here is a `String`/`Option`/`Vec`), so the error arm exists
/// for soundness rather than an expected path.
fn json_response(feed: &opds2::Feed) -> Response {
    match serde_json::to_string(feed) {
        Ok(body) => ([(header::CONTENT_TYPE, opds2::MEDIA_TYPE)], body).into_response(),
        Err(e) => internal("serialize opds2 feed", e),
    }
}

/// A navigation [`opds2::Link`] — the JSON counterpart to [`nav_entry`]:
/// `rel="subsection"` into another feed, with the target's title inline
/// (JSON navigation has no separate `<summary>` slot the way Atom's
/// `<entry>` does).
fn json_nav_link(title: &str, href: &str) -> opds2::Link {
    opds2::Link::new(href)
        .with_rel("subsection")
        .with_type(opds2::MEDIA_TYPE)
        .with_title(title)
}
