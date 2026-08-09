//! Letter-indexed author browse (AC2): `/opds/authors` (the letters that
//! have at least one author), `/opds/authors/{letter}` (authors under that
//! letter), and `/opds/author/{id}` (one author's acquisition feed).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use omnibus_shared::AuthorSummary;

use super::atom::{Feed, Link, ACQUISITION_TYPE, NAVIGATION_TYPE};
use super::entries::book_entry;
use super::{internal, nav_entry, now_rfc3339, xml_response};
use crate::auth::AuthUser;
use crate::backend::AppState;

/// The bucket an author's sort key falls into: an uppercase ASCII letter,
/// or `#` for anything that doesn't start with one (numerals, symbols,
/// non-Latin scripts) — the same "everything else" bucket most library
/// apps use for a letter-indexed browse. `pub(super)` so `opds::json_authors`
/// buckets authors identically for the JSON catalog's letter index.
pub(super) fn letter_of(author: &AuthorSummary) -> char {
    let key = author.sort.as_deref().unwrap_or(&author.name);
    key.chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .filter(char::is_ascii_alphabetic)
        .unwrap_or('#')
}

/// URL-path form of a bucket letter. `#` starts a fragment and is never
/// sent to the server, so the "everything else" bucket must be
/// percent-encoded (`%23`) in any `href` — the letter itself (used for
/// display text and ids) stays literal. `pub(super)` — shared with
/// `opds::json_authors`.
pub(super) fn letter_path_segment(letter: char) -> String {
    if letter == '#' {
        "%23".to_string()
    } else {
        letter.to_string()
    }
}

/// `GET /opds/authors` — navigation feed of the letters that have at least
/// one author, each linking to `/opds/authors/{letter}`.
pub(super) async fn letter_index(_user: AuthUser, State(state): State<AppState>) -> Response {
    let authors = match load_authors(&state).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let mut letters: Vec<char> = authors.iter().map(letter_of).collect();
    letters.sort_unstable();
    letters.dedup();
    let updated = now_rfc3339();
    let entries = letters
        .into_iter()
        .map(|letter| {
            nav_entry(
                &format!("urn:omnibus:opds:authors:{letter}"),
                &letter.to_string(),
                &updated,
                &format!("/opds/authors/{}", letter_path_segment(letter)),
                NAVIGATION_TYPE,
                &format!("Authors starting with {letter}"),
            )
        })
        .collect();
    let feed = Feed {
        id: "urn:omnibus:opds:authors".to_string(),
        title: "Authors".to_string(),
        updated,
        links: vec![
            Link::new("self", "/opds/authors", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries,
    };
    xml_response(NAVIGATION_TYPE, feed.to_xml())
}

/// `GET /opds/authors/{letter}` — navigation feed of authors whose bucket
/// (see [`letter_of`]) is `letter`, each linking to its `/opds/author/{id}`
/// acquisition feed. An empty or non-alphabetic `letter` 400s rather than
/// silently matching the `#` bucket, so a malformed link is visible.
pub(super) async fn by_letter(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(letter): Path<String>,
) -> Response {
    let mut chars = letter.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return (StatusCode::BAD_REQUEST, "letter must be a single character").into_response();
    };
    let target = if first.is_ascii_alphabetic() {
        first.to_ascii_uppercase()
    } else if first == '#' {
        '#'
    } else {
        return (StatusCode::BAD_REQUEST, "letter must be A-Z or #").into_response();
    };
    let authors = match load_authors(&state).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let updated = now_rfc3339();
    let entries = authors
        .iter()
        .filter(|a| letter_of(a) == target)
        .map(|a| {
            nav_entry(
                &format!("urn:omnibus:opds:author:{}", a.id),
                &a.name,
                &updated,
                &format!("/opds/author/{}", a.id),
                ACQUISITION_TYPE,
                &format!("{} book(s)", a.book_count),
            )
        })
        .collect();
    let feed = Feed {
        id: format!("urn:omnibus:opds:authors:{target}"),
        title: format!("Authors: {target}"),
        updated,
        links: vec![
            Link::new(
                "self",
                format!("/opds/authors/{}", letter_path_segment(target)),
                NAVIGATION_TYPE,
            ),
            Link::new("up", "/opds/authors", NAVIGATION_TYPE),
            Link::new("start", "/opds", NAVIGATION_TYPE),
        ],
        entries,
    };
    xml_response(NAVIGATION_TYPE, feed.to_xml())
}

/// `GET /opds/author/{id}` — acquisition feed of one author's books.
pub(super) async fn acquisition_feed(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_author(&state.pool, id).await {
        Ok(Some(author)) => {
            // `db::get_author` returns books across every library, but this
            // catalog is ebook-scoped (see the module doc); an audiobook-only
            // or physical-only row has no epub/cbz format and would otherwise
            // surface as an entry with no acquisition link.
            let is_ebook_format =
                |f: &String| f.eq_ignore_ascii_case("epub") || f.eq_ignore_ascii_case("cbz");
            let feed = Feed {
                id: format!("urn:omnibus:opds:author:{id}"),
                title: author.name.clone(),
                updated: now_rfc3339(),
                links: vec![
                    Link::new("self", format!("/opds/author/{id}"), ACQUISITION_TYPE),
                    Link::new("start", "/opds", NAVIGATION_TYPE),
                ],
                entries: author
                    .books
                    .iter()
                    .filter(|b| b.formats.iter().any(is_ebook_format))
                    .map(book_entry)
                    .collect(),
            };
            xml_response(ACQUISITION_TYPE, feed.to_xml())
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("read author", e),
    }
}

/// Load every author across the configured ebook library (see the `opds`
/// module doc for why the browse is ebook-scoped), or `Err` with the
/// ready-to-return failure response. `pub(super)` — reused by
/// `opds::json_authors` so the Atom and JSON letter indexes read the exact
/// same author set.
pub(super) async fn load_authors(state: &AppState) -> Result<Vec<AuthorSummary>, Response> {
    let settings = db::get_settings(&state.pool)
        .await
        .map_err(|e| internal("read settings", e))?;
    let paths = db::collect_paths(settings.ebook_library_path.as_deref(), None);
    db::list_authors(&state.pool, &paths)
        .await
        .map_err(|e| internal("list authors", e))
}
