//! Override-aware detection: a duplicate no book effectively carries is
//! ignored, a book an override reassigned counts, removed tags and
//! replaced series are skipped, and an already-overridden title is left
//! alone while other overridden fields do not suppress it.

use sqlx::SqlitePool;

use super::super::*;
use super::{
    insert_author, insert_book, insert_series, insert_tag, link_author, link_series, link_tag,
    merge_payload, new_pool, seed_root,
};

/// Save a raw overrides JSON blob against a book uuid.
async fn set_overrides(pool: &SqlitePool, uuid: &str, overrides_json: &str) {
    sqlx::query("INSERT INTO metadata_overrides (book_uuid, overrides) VALUES (?, json(?))")
        .bind(uuid)
        .bind(overrides_json)
        .execute(pool)
        .await
        .unwrap();
}

// Override-aware detection
#[tokio::test]
async fn detect_authors_ignores_a_duplicate_no_book_effectively_carries() {
    // The link rows survive a creators override, so detecting off them alone
    // proposes merging an author the app no longer shows anywhere.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_author(&pool, "Brandon Sanderson", None).await;
    let duplicate = insert_author(&pool, "Sanderson, Brandon", None).await;
    let kept = insert_book(&pool, lib, "u1", "Book One").await;
    let overridden = insert_book(&pool, lib, "u2", "Book Two").await;
    link_author(&pool, kept, canonical).await;
    link_author(&pool, overridden, duplicate).await;
    set_overrides(
        &pool,
        "u2",
        r#"{"creators":[{"name":"Brandon Sanderson","role":null,"file_as":null}]}"#,
    )
    .await;

    let suggestions = detect_authors(&pool).await.unwrap();
    assert!(
        suggestions.is_empty(),
        "the duplicate is carried by no book, so there is nothing to merge: {suggestions:?}"
    );
}

#[tokio::test]
async fn detect_authors_counts_a_book_an_override_reassigned_to_the_author() {
    // The other direction: an override *adds* a membership the link table
    // never had, and the affected-book count has to see it.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_author(&pool, "Brandon Sanderson", None).await;
    let duplicate = insert_author(&pool, "Sanderson, Brandon", None).await;
    let linked = insert_book(&pool, lib, "u1", "Book One").await;
    link_author(&pool, linked, duplicate).await;
    let reassigned = insert_book(&pool, lib, "u2", "Book Two").await;
    link_author(&pool, reassigned, duplicate).await;
    set_overrides(
        &pool,
        "u2",
        r#"{"creators":[{"name":"Brandon Sanderson","role":null,"file_as":null}]}"#,
    )
    .await;

    let suggestions = detect_authors(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].book_count, 2);
    let (source_ids, _, canonical_id, _) = merge_payload(&suggestions[0]);
    // One effective book each — the override moved u2 from `duplicate` to
    // `canonical` — so the tie breaks toward the lower id.
    assert_eq!(canonical_id, canonical);
    assert_eq!(source_ids, [duplicate]);
}

#[tokio::test]
async fn detect_tags_merge_ignores_a_tag_a_subjects_override_removed() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_tag(&pool, "Sci-Fi").await;
    let duplicate = insert_tag(&pool, "Sci Fi").await;
    let kept = insert_book(&pool, lib, "u1", "Book One").await;
    let overridden = insert_book(&pool, lib, "u2", "Book Two").await;
    link_tag(&pool, kept, canonical).await;
    link_tag(&pool, overridden, duplicate).await;
    set_overrides(&pool, "u2", r#"{"subjects":["Sci-Fi"]}"#).await;

    let suggestions = detect_tags_merge(&pool).await.unwrap();
    assert!(suggestions.is_empty(), "got {suggestions:?}");
}

#[tokio::test]
async fn detect_series_ignores_a_duplicate_a_series_override_replaced() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_series(&pool, "The Stormlight Archive", None).await;
    let duplicate = insert_series(&pool, "Stormlight Archive, The", None).await;
    let kept = insert_book(&pool, lib, "u1", "Book One").await;
    let overridden = insert_book(&pool, lib, "u2", "Book Two").await;
    link_series(&pool, kept, canonical).await;
    link_series(&pool, overridden, duplicate).await;
    set_overrides(&pool, "u2", r#"{"series":"The Stormlight Archive"}"#).await;

    let suggestions = detect_series(&pool).await.unwrap();
    assert!(suggestions.is_empty(), "got {suggestions:?}");
}

#[tokio::test]
async fn detect_book_titles_skips_a_book_whose_title_is_already_overridden() {
    // The scanned title never changes, so without this the same rename card
    // came back after every pass — including for a book renamed by accepting
    // that very card.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    insert_book(&pool, lib, "u1", "Maas, Sarah J - A Court of Mist and Fury").await;
    insert_book(
        &pool,
        lib,
        "u2",
        "Maas, Sarah J - A Court of Wings and Ruin",
    )
    .await;
    set_overrides(&pool, "u2", r#"{"title":"A Court of Wings and Ruin"}"#).await;

    let suggestions = detect_book_titles(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0].primary_name,
        "Maas, Sarah J - A Court of Mist and Fury"
    );
}

#[tokio::test]
async fn detect_book_titles_still_fires_for_a_book_overriding_another_field() {
    // Only a *title* override is the authority on the title; a description
    // edit must not immunize the book against a rename suggestion.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    insert_book(&pool, lib, "u1", "Maas, Sarah J - A Court of Mist and Fury").await;
    set_overrides(&pool, "u1", r#"{"description":"Edited blurb"}"#).await;

    let suggestions = detect_book_titles(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0].secondary_name.as_deref(),
        Some("A Court of Mist and Fury")
    );
}
