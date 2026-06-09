//! Unit tests for the `browse` module — `/authors` and `/series` index
//! pages: alpha-ordering, override-aware counts, audiobook-library scope,
//! and `has_photo` propagation.

use super::*;
use crate::author_photos_data::{upsert_author_photo, AuthorPhotoSource};
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::test_support::*;
use omnibus_shared::{Contributor, MetadataOverrides};

// -----------------------------------------------------------------
// F1.12 index pages — list_authors / list_series
// -----------------------------------------------------------------
#[tokio::test]
async fn list_authors_returns_all_with_counts_and_alpha_order() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let authors = list_authors(&pool, &["/lib"]).await.unwrap();

    // Three distinct authors: Ada Lovelace, Grace Hopper, Niklaus Wirth.
    let names: Vec<_> = authors.iter().map(|a| a.name.clone()).collect();
    assert_eq!(
        names,
        vec![
            "Ada Lovelace".to_string(),
            "Grace Hopper".to_string(),
            "Niklaus Wirth".to_string(),
        ],
        "expected NOCASE alphabetical order by sort/name"
    );

    // Book counts: Ada=3, Grace=1, Niklaus=1.
    let by_name: std::collections::HashMap<_, _> = authors
        .iter()
        .map(|a| (a.name.clone(), a.book_count))
        .collect();
    assert_eq!(by_name["Ada Lovelace"], 3);
    assert_eq!(by_name["Grace Hopper"], 1);
    assert_eq!(by_name["Niklaus Wirth"], 1);

    // IDs are populated so cards can route to /authors/:id.
    assert!(authors.iter().all(|a| a.id > 0));
}
#[tokio::test]
async fn list_authors_scopes_to_library_path() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let authors = list_authors(&pool, &["/no-such-library"]).await.unwrap();
    assert!(
        authors.is_empty(),
        "unknown library path must yield empty list"
    );
}
#[tokio::test]
async fn list_authors_returns_empty_for_empty_library() {
    let _guard = CoversTempDir::new("empty_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let authors = list_authors(&pool, &["/lib"]).await.unwrap();
    assert!(authors.is_empty());
}
#[tokio::test]
async fn list_series_returns_all_with_counts_and_alpha_order() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let series = list_series(&pool, &["/lib"]).await.unwrap();

    // Two series: Pioneers, Saga (NOCASE alpha).
    let names: Vec<_> = series.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec!["Pioneers".to_string(), "Saga".to_string()]);

    let by_name: std::collections::HashMap<_, _> = series
        .iter()
        .map(|s| (s.name.clone(), s.book_count))
        .collect();
    assert_eq!(by_name["Saga"], 2);
    assert_eq!(by_name["Pioneers"], 1);
}
#[tokio::test]
async fn list_series_populates_primary_author_from_first_book() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let series = list_series(&pool, &["/lib"]).await.unwrap();

    let by_name: std::collections::HashMap<_, _> = series
        .iter()
        .map(|s| (s.name.clone(), s.primary_author.clone()))
        .collect();
    // Saga book one's first creator is "Ada Lovelace" (the two-author
    // book lists Ada first); Pioneers has Niklaus Wirth as sole author.
    assert_eq!(by_name["Saga"], Some("Ada Lovelace".to_string()));
    assert_eq!(by_name["Pioneers"], Some("Niklaus Wirth".to_string()));
}
#[tokio::test]
async fn list_series_scopes_to_library_path() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let series = list_series(&pool, &["/no-such-library"]).await.unwrap();
    assert!(series.is_empty());
}
// F5.1 — index-page counts must follow the same override overlay
// applied to `/authors/:id` and `/series/:id` in PR #153. Without
// these, an author whose books were reassigned through the edit
// form still reports the canonical count on /authors, then
// /authors/:id shows the corrected list — a visible inconsistency.
#[tokio::test]
async fn list_authors_book_count_follows_override_creators() {
    // Setup: Ada has 3 canonical books, Grace has 1 (saga1 lists
    // both, with Ada first). Override saga2 so its single creator
    // is Grace instead of Ada. Expected effective counts:
    //   Ada   → 2 (saga1 + standalone; saga2 reassigned away)
    //   Grace → 2 (saga1 still + saga2 from override)
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let books = list_books(&pool, "/lib").await.unwrap();
    let saga2 = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
    let uuid = saga2.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Grace Hopper".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let authors = list_authors(&pool, &["/lib"]).await.unwrap();
    let by_name: std::collections::HashMap<_, _> = authors
        .iter()
        .map(|a| (a.name.clone(), a.book_count))
        .collect();
    assert_eq!(
        by_name.get("Ada Lovelace").copied(),
        Some(2),
        "Ada loses saga2 to the override",
    );
    assert_eq!(
        by_name.get("Grace Hopper").copied(),
        Some(2),
        "Grace picks up saga2 from the override",
    );
}
#[tokio::test]
async fn list_authors_book_count_matches_canonical_creator_case_insensitively() {
    // `authors.name` is `UNIQUE COLLATE NOCASE`; an override that
    // differs only by case ("ada lovelace") still resolves to the
    // canonical "Ada Lovelace" row. Mirrors the NOCASE follow-up
    // applied to /author/:id (commit aca8a81b).
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let books = list_books(&pool, "/lib").await.unwrap();
    let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
    let uuid = other.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "ADA LOVELACE".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let authors = list_authors(&pool, &["/lib"]).await.unwrap();
    let ada = authors
        .iter()
        .find(|a| a.name == "Ada Lovelace")
        .expect("Ada present");
    assert_eq!(
        ada.book_count, 4,
        "case-mismatched override should still increment canonical Ada's count",
    );
}
#[tokio::test]
async fn list_series_book_count_follows_override_series() {
    // Move the standalone book into Saga via override. Expected:
    // Saga's count goes from 2 → 3. (The canonical books_series_link
    // is untouched; the overlay surfaces the effective membership.)
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some("3".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = list_series(&pool, &["/lib"]).await.unwrap();
    let saga = series
        .iter()
        .find(|s| s.name == "Saga")
        .expect("Saga present");
    assert_eq!(saga.book_count, 3, "override should add standalone to Saga");
}
#[tokio::test]
async fn list_series_primary_author_follows_override_creators() {
    // Saga's first book (saga1.epub by series_index) has canonical
    // creators [Ada Lovelace, Grace Hopper] — primary_author is
    // "Ada Lovelace". Override the first creator to "Margaret
    // Hamilton"; the index by-line should follow.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let books = list_books(&pool, "/lib").await.unwrap();
    let saga1 = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga1.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Margaret Hamilton".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = list_series(&pool, &["/lib"]).await.unwrap();
    let saga = series
        .iter()
        .find(|s| s.name == "Saga")
        .expect("Saga present");
    assert_eq!(
        saga.primary_author.as_deref(),
        Some("Margaret Hamilton"),
        "override creator drives the index by-line",
    );
}
/// `list_authors` mirrors the `get_author` has_photo semantics so the
/// /authors index can pick the right avatar without a per-card detail
/// fetch. Same three-state matrix: no row → false, `letter` marker →
/// false, `manual` upload → true.
#[tokio::test]
async fn list_authors_populates_has_photo() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let initial = list_authors(&pool, &["/lib"]).await.unwrap();
    let ada = initial.iter().find(|a| a.id == ada_id).unwrap();
    assert!(!ada.has_photo, "no row should yield has_photo = false");

    upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();
    let after_letter = list_authors(&pool, &["/lib"]).await.unwrap();
    let ada = after_letter.iter().find(|a| a.id == ada_id).unwrap();
    assert!(
        !ada.has_photo,
        "letter marker should yield has_photo = false"
    );

    upsert_author_photo(
        &pool,
        ada_id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/jpeg"),
        Some(b"\xFF\xD8\xFFfake"),
    )
    .await
    .unwrap();
    let after_upload = list_authors(&pool, &["/lib"]).await.unwrap();
    let ada = after_upload.iter().find(|a| a.id == ada_id).unwrap();
    assert!(ada.has_photo, "manual upload should yield has_photo = true");
}

// -----------------------------------------------------------------
// Audiobook authors/series in browse indexes
// -----------------------------------------------------------------

#[tokio::test]
async fn list_authors_includes_audiobook_authors_from_separate_library() {
    let guard = CoversTempDir::new("ab_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::sync::replace_books(
        &pool,
        "/ebooks",
        vec![indexed(
            "novel.epub",
            Some("A Novel"),
            &["Ebook Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    crate::sync::sync_audiobooks(
        &pool,
        "/audiobooks",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![indexed_audiobook(
                "narrator/book",
                "Audiobook Title",
                Some("Audio Author"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let ebook_only = list_authors(&pool, &["/ebooks"]).await.unwrap();
    assert_eq!(ebook_only.len(), 1);
    assert_eq!(ebook_only[0].name, "Ebook Author");

    let audio_only = list_authors(&pool, &["/audiobooks"]).await.unwrap();
    assert_eq!(audio_only.len(), 1);
    assert_eq!(audio_only[0].name, "Audio Author");

    let combined = list_authors(&pool, &["/ebooks", "/audiobooks"])
        .await
        .unwrap();
    let names: Vec<_> = combined.iter().map(|a| a.name.clone()).collect();
    assert!(names.contains(&"Audio Author".to_string()));
    assert!(names.contains(&"Ebook Author".to_string()));
    assert_eq!(combined.len(), 2);
    drop(guard);
}

#[tokio::test]
async fn list_authors_returns_empty_for_no_paths() {
    let _guard = CoversTempDir::new("no_paths");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let authors = list_authors(&pool, &[]).await.unwrap();
    assert!(authors.is_empty());
}

#[tokio::test]
async fn list_series_includes_series_from_audiobook_library() {
    let guard = CoversTempDir::new("ab_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::sync::replace_books(
        &pool,
        "/ebooks",
        vec![indexed(
            "dune.epub",
            Some("Dune"),
            &["Frank Herbert"],
            &[],
            Some(("Dune Chronicles", "1")),
            None,
        )],
    )
    .await
    .unwrap();

    // Seed an audiobook under a separate library, then add a series
    // override that references the existing "Dune Chronicles" series.
    crate::sync::sync_audiobooks(
        &pool,
        "/audiobooks",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![indexed_audiobook(
                "herbert/dune-audio",
                "Dune (Audio)",
                Some("Frank Herbert"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    upsert_metadata_overrides(
        &pool,
        "ab-uuid-herbert-dune-audio",
        &MetadataOverrides {
            series: Some("Dune Chronicles".into()),
            series_index: Some("1".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let ebook_series = list_series(&pool, &["/ebooks"]).await.unwrap();
    assert_eq!(ebook_series.len(), 1);
    assert_eq!(ebook_series[0].name, "Dune Chronicles");

    let combined = list_series(&pool, &["/ebooks", "/audiobooks"])
        .await
        .unwrap();
    assert_eq!(combined.len(), 1, "same series should not duplicate");
    assert_eq!(
        combined[0].book_count, 2,
        "audiobook with series override should be counted"
    );
    drop(guard);
}
