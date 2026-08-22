//! `/opds/*` — an OPDS 1.2 Atom catalog for third-party e-reader clients — and
//! `/opds/v2/*`, its OPDS 2.0 JSON counterpart, whose `json_*` submodules mirror
//! the Atom ones one-for-one and reuse their DB reads so the two can't drift.
//! Both authenticate via [`crate::auth::OpdsAuthUser`] (session or HTTP Basic),
//! are ebook-library scoped, and hand out byte links on the [`delegate`] routes.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use omnibus_db as db;
use omnibus_shared::opds as opds2;
use omnibus_shared::EbookMetadata;

use crate::auth::OpdsAuthUser;
use crate::http_errors::internal;

use super::AppState;

mod all;
mod atom;
mod authors;
mod delegate;
mod entries;
mod json_all;
mod json_authors;
mod json_entries;
mod json_nav;
mod json_new;
mod json_search;
mod json_series;
mod json_shelves;
mod nav;
mod new;
mod search;
mod series;
mod shelves;
#[cfg(test)]
mod tests;

/// Build the `/opds/*` router. `Extension(pool)` (and the Basic-auth
/// state behind [`crate::auth::OpdsAuthUser`]) are layered here so the
/// router is self-contained for integration tests, mirroring `kobo_router`
/// in `super::kobo`; the live server layers the same pool at the top
/// (harmless overlap, mirroring `rest_router`).
pub fn opds_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/opds", get(nav::root))
        .route("/opds/osd", get(nav::osd))
        .route("/opds/search", get(search::search))
        .route("/opds/new", get(new::new_arrivals))
        .route("/opds/all", get(all::all_books))
        .route("/opds/authors", get(authors::letter_index))
        .route("/opds/authors/{letter}", get(authors::by_letter))
        .route("/opds/author/{id}", get(authors::acquisition_feed))
        .route("/opds/series", get(series::index))
        .route("/opds/series/{id}", get(series::acquisition_feed))
        .route("/opds/shelves", get(shelves::index))
        .route("/opds/shelves/{id}", get(shelves::acquisition_feed))
        .route("/opds/v2", get(json_nav::root))
        .route("/opds/v2/search", get(json_search::search))
        .route("/opds/v2/new", get(json_new::new_arrivals))
        .route("/opds/v2/all", get(json_all::all_books))
        .route("/opds/v2/authors", get(json_authors::letter_index))
        .route("/opds/v2/authors/{letter}", get(json_authors::by_letter))
        .route("/opds/v2/author/{id}", get(json_authors::acquisition_feed))
        .route("/opds/v2/series", get(json_series::index))
        .route("/opds/v2/series/{id}", get(json_series::acquisition_feed))
        .route("/opds/v2/shelves", get(json_shelves::index))
        .route("/opds/v2/shelves/{id}", get(json_shelves::acquisition_feed))
        .route("/opds/covers/{uuid}", get(delegate::cover))
        .route("/opds/thumbs/{uuid}/{size}", get(delegate::thumb))
        .route("/opds/ebooks/{uuid}/file", get(delegate::ebook_file))
        .route(
            "/opds/ebooks/{uuid}/download",
            get(delegate::ebook_download),
        )
        .route(
            "/opds/audiobooks/{uuid}/download",
            get(delegate::audiobook_download),
        )
        .with_state(state)
        .layer(Extension(pool))
        .layer(Extension(crate::auth::BasicAuthState::new_shared()))
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
/// the JSON analog of [`xml_response`]. Builders are responsible for keeping
/// `Feed` serializable (e.g. `json_entries::series_ref` filters
/// `series_index` to finite floats before it ever reaches this point), so
/// the error arm exists for soundness rather than an expected path.
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

/// Drop every book in `books` that's confined to a manual shelf `user`
/// cannot see: on at least one manual shelf, and every one of those
/// shelves is private and owned by someone else. A book on no shelf at
/// all, or on any shelf visible to `user`, is untouched — so this only
/// ever narrows a page the caller already fetched through the normal
/// (shelf-unaware) library read. Every browse/search/new/nav handler in
/// both catalogs runs its page through this after the e-reader-format
/// filter, so the two catalogs can't drift on the rule.
///
/// **Known gap:** the per-author/per-series *navigation*-level `book_count`
/// shown in `/opds/authors/{letter}` and `/opds/series` (sourced from
/// `db::list_authors`/`list_series`, informational summary text only) is not
/// narrowed by this filter — only the acquisition-feed entries are. A
/// shelf-exclusive book can still nudge that count without ever appearing.
async fn retain_shelf_visible(
    state: &AppState,
    user: &OpdsAuthUser,
    books: &mut Vec<EbookMetadata>,
) -> Result<(), Response> {
    let candidates: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let hidden = db::shelves::shelf_exclusive_hidden_uuids(
        &state.pool,
        user.0.id,
        user.0.is_admin,
        &candidates,
    )
    .await
    .map_err(|e| internal("filter shelf-exclusive books", e))?;
    if !hidden.is_empty() {
        books.retain(|b| match &b.unique_identifier {
            Some(uuid) => !hidden.contains(uuid),
            None => true,
        });
    }
    Ok(())
}

/// Single-uuid form of [`retain_shelf_visible`] for the byte-serving
/// [`delegate`] routes, which resolve a `{uuid}` path segment directly
/// rather than paging through a fetched list. `true` means `user` must not
/// be shown this book — the delegate routes 404 rather than 403, matching
/// [`shelves::load_visible_shelf`]'s no-existence-leak rule.
///
/// Canonicalizes `uuid` through [`db::resolve_canonical_book_uuid`] first
/// (#932 follow-up): the media handlers this gate sits in front of resolve
/// a path uuid via `resolve_book_id_by_uuid`, which falls back to
/// `merged_uuids` — a book merged away from an old uuid still serves under
/// it. `shelf_books` only ever names the *canonical* uuid, so checking the
/// raw path segment let an old/merged uuid walk straight past a shelf that
/// hides the surviving book. An unresolvable uuid (unknown to both `books`
/// and `merged_uuids`) is never "shelf-hidden" by this rule — the handler's
/// own lookup 404s it as not-found, which is the correct answer for that
/// case.
async fn is_shelf_hidden(
    state: &AppState,
    user: &OpdsAuthUser,
    uuid: &str,
) -> Result<bool, Response> {
    let Some(canonical) = db::resolve_canonical_book_uuid(&state.pool, uuid)
        .await
        .map_err(|e| internal("resolve canonical book uuid", e))?
    else {
        return Ok(false);
    };
    let candidates = [canonical.clone()];
    let hidden = db::shelves::shelf_exclusive_hidden_uuids(
        &state.pool,
        user.0.id,
        user.0.is_admin,
        &candidates,
    )
    .await
    .map_err(|e| internal("check shelf-exclusive visibility", e))?;
    Ok(hidden.contains(&canonical))
}

/// Shared guard for the byte-serving [`delegate`] routes: collapses the
/// `is_shelf_hidden` match block duplicated across all five of them into
/// one call. Returns `Some(response)` to short-circuit the caller (a 404
/// for a hidden book, or the propagated error), `None` to continue past
/// the guard into the `/api` handler body.
async fn guard_shelf_hidden(state: &AppState, user: &OpdsAuthUser, uuid: &str) -> Option<Response> {
    match is_shelf_hidden(state, user, uuid).await {
        Ok(true) => Some(StatusCode::NOT_FOUND.into_response()),
        Ok(false) => None,
        Err(resp) => Some(resp),
    }
}
