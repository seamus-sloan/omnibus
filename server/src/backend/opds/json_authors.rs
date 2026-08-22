//! Letter-indexed author browse (AC2), the OPDS 2.0 JSON counterpart to
//! `/opds/authors*`: `/opds/v2/authors`, `/opds/v2/authors/{letter}`, and
//! `/opds/v2/author/{id}`. Reuses `authors::{load_author, load_authors,
//! letter_of, letter_path_segment}` so both catalogs bucket the same author
//! under the same letter and read the same author set.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use omnibus_shared::opds::{Feed, FeedMetadata, Link, MEDIA_TYPE};

use super::authors::{letter_of, letter_path_segment, load_author, load_authors};
use super::json_entries::book_publication;
use super::{json_nav_link, json_response};
use crate::auth::OpdsAuthUser;
use crate::backend::AppState;

/// `GET /opds/v2/authors` — navigation feed of the letters that have at
/// least one author, each linking to `/opds/v2/authors/{letter}`.
pub(super) async fn letter_index(_user: OpdsAuthUser, State(state): State<AppState>) -> Response {
    let authors = match load_authors(&state).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let mut letters: Vec<char> = authors.iter().map(letter_of).collect();
    letters.sort_unstable();
    letters.dedup();
    let navigation = letters
        .into_iter()
        .map(|letter| {
            json_nav_link(
                &letter.to_string(),
                &format!("/opds/v2/authors/{}", letter_path_segment(letter)),
            )
        })
        .collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: "Authors".to_string(),
            ..Default::default()
        },
        links: vec![
            Link::new("/opds/v2/authors")
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

/// `GET /opds/v2/authors/{letter}` — navigation feed of authors whose
/// bucket is `letter`, each linking to its `/opds/v2/author/{id}`
/// publication feed. Same validation as the Atom equivalent: an empty or
/// non-alphabetic `letter` 400s rather than silently matching `#`.
pub(super) async fn by_letter(
    _user: OpdsAuthUser,
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
    let navigation = authors
        .iter()
        .filter(|a| letter_of(a) == target)
        .map(|a| json_nav_link(&a.name, &format!("/opds/v2/author/{}", a.id)))
        .collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: format!("Authors: {target}"),
            ..Default::default()
        },
        links: vec![
            Link::new(format!("/opds/v2/authors/{}", letter_path_segment(target)))
                .with_rel("self")
                .with_type(MEDIA_TYPE),
            Link::new("/opds/v2/authors")
                .with_rel("up")
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

/// `GET /opds/v2/author/{id}` — publication feed of one author's books.
pub(super) async fn acquisition_feed(
    user: OpdsAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    let author = match load_author(&state, &user, id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(resp) => return resp,
    };
    let publications: Vec<_> = author.books.iter().map(book_publication).collect();
    let feed = Feed {
        metadata: FeedMetadata {
            title: author.name.clone(),
            number_of_items: Some(publications.len() as i64),
            ..Default::default()
        },
        links: vec![
            Link::new(format!("/opds/v2/author/{id}"))
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
