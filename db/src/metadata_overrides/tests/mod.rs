//! Tests for the metadata-overrides write path (upsert/merge/get/delete),
//! its FTS rebuild, and the series-link / tag-link materialization. The
//! shared seed helpers and the `override_match_keys` normalization unit
//! tests live here; the rest is split into the sibling modules below.

mod bulk;
mod cover;
mod crud;
mod fts;
mod genres;
mod links;
mod precedence;

use omnibus_shared::{Contributor, MetadataOverrides};

use crate::books::list_books;
use crate::sync::replace_books;
use crate::test_support::indexed;

use super::upsert::override_match_keys;

/// Count `tags` rows matching `name` (NOCASE column, so case variants match).
async fn tag_row_count(pool: &sqlx::SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Seed one scanned book under `/lib` with the given canonical tags and
/// return its `(uuid, id)`.
async fn seed_one_book_with_tags(pool: &sqlx::SqlitePool, tags: &[&str]) -> (String, i64) {
    replace_books(
        pool,
        "/lib",
        vec![indexed(
            "book.epub",
            Some("Book"),
            &["Author"],
            tags,
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let books = list_books(pool, "/lib").await.unwrap();
    (books[0].unique_identifier.clone().unwrap(), books[0].id)
}

// -----------------------------------------------------------------
// Genres — override-only, no scanned baseline (migration 0066)
// -----------------------------------------------------------------

fn with_genres(genres: &[&str]) -> MetadataOverrides {
    MetadataOverrides {
        genres: Some(genres.iter().map(|g| (*g).to_string()).collect()),
        ..Default::default()
    }
}

async fn genre_row_names(pool: &sqlx::SqlitePool) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM genres ORDER BY name")
        .fetch_all(pool)
        .await
        .unwrap()
}

// -----------------------------------------------------------------
// `override_match_keys` (Physical Check-In's fuzzy-rung match keys)
// -----------------------------------------------------------------

/// A normal override with both a title and a first creator normalizes both
/// sides, mirroring the sync writer's derivation of `books.(title_norm,
/// author_norm)` from scanned metadata.
#[test]
fn override_match_keys_normalizes_title_and_first_creator_when_both_set() {
    let ov = MetadataOverrides {
        title: Some("The Great Gatsby".into()),
        creators: Some(vec![
            Contributor {
                name: "F. Scott Fitzgerald".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            },
            Contributor {
                name: "Someone Else".into(),
                role: Some("edt".into()),
                file_as: None,
                id: None,
            },
        ]),
        ..Default::default()
    };
    let (title_norm, author_norm) = override_match_keys(&ov);
    assert_eq!(title_norm.as_deref(), Some("the great gatsby"));
    assert_eq!(
        author_norm.as_deref(),
        Some("f scott fitzgerald"),
        "only the first creator should feed the author match key"
    );
}

/// `title: Some("")` is the documented "clear" sentinel (mirrors
/// `apply_overrides`'s ISBN handling): it must normalize to `None`, not
/// `Some("")`, so the resolver falls back to the scanned `title_norm`
/// instead of matching against an empty string.
#[test]
fn override_match_keys_normalizes_empty_string_clear_sentinel_to_none() {
    let ov = MetadataOverrides {
        title: Some(String::new()),
        creators: Some(vec![Contributor {
            name: String::new(),
            role: None,
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    let (title_norm, author_norm) = override_match_keys(&ov);
    assert_eq!(title_norm, None);
    assert_eq!(author_norm, None);
}

/// No override set for either field passes through as `(None, None)`,
/// signalling the resolver to fall back entirely to the scanned norms.
#[test]
fn override_match_keys_returns_none_for_both_when_no_override_set() {
    let ov = MetadataOverrides::default();
    let (title_norm, author_norm) = override_match_keys(&ov);
    assert_eq!(title_norm, None);
    assert_eq!(author_norm, None);
}
