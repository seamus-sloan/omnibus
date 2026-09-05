//! Each `detect_*` entry point against a seeded pool: the Tier-0 and
//! Tier-1 branches of the author, series, tag-merge, tag-split and
//! book-title detectors, `detect_all`'s dispatch, and every
//! `CleanupError::Db` propagation.

use sqlx::SqlitePool;

use super::super::*;
use super::{
    insert_author, insert_book, insert_series, insert_tag, link_author, link_series, link_tag,
    merge_payload, new_pool, seed_root,
};

/// Give each of `author_ids` a book of its own.
///
/// Detection only considers taxonomy at least one book effectively carries,
/// so a fixture of bare `authors` rows detects nothing. One book each also
/// keeps the book counts equal, which leaves the canonical tie-break on the
/// lowest id — the property most of these tests actually assert.
async fn book_each_author(pool: &SqlitePool, lib: i64, author_ids: &[i64]) {
    for (n, id) in author_ids.iter().enumerate() {
        let book = insert_book(pool, lib, &format!("author-u{n}"), &format!("Book {n}")).await;
        link_author(pool, book, *id).await;
    }
}

/// [`book_each_author`] for tags.
async fn book_each_tag(pool: &SqlitePool, lib: i64, tag_ids: &[i64]) {
    for (n, id) in tag_ids.iter().enumerate() {
        let book = insert_book(pool, lib, &format!("tag-u{n}"), &format!("Tagged {n}")).await;
        link_tag(pool, book, *id).await;
    }
}

// detect_authors
#[tokio::test]
async fn detect_authors_collapses_last_first_swap_into_one_tier0_merge_suggestion() {
    // AC1: `Brandon Sanderson` / `Sanderson, Brandon` collapse via the
    // normalized-key swap.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_author(&pool, "Brandon Sanderson", None).await;
    let duplicate = insert_author(&pool, "Sanderson, Brandon", None).await;
    let b1 = insert_book(&pool, lib, "u1", "Book One").await;
    let b2 = insert_book(&pool, lib, "u2", "Book Two").await;
    let b3 = insert_book(&pool, lib, "u3", "Book Three").await;
    link_author(&pool, b1, canonical).await;
    link_author(&pool, b2, canonical).await;
    link_author(&pool, b3, duplicate).await;

    let suggestions = detect_authors(&pool).await.unwrap();
    let merges: Vec<_> = suggestions
        .iter()
        .filter(|s| s.action == CleanupAction::Merge)
        .collect();
    assert_eq!(merges.len(), 1);
    let s = merges[0];
    assert_eq!(s.kind, CleanupKind::Author);
    assert_eq!(s.tier, Tier::Zero);
    assert_eq!(s.score, 1.0);
    assert_eq!(s.book_count, 3);
    assert_eq!(s.primary_name, "Brandon Sanderson");
    assert_eq!(s.secondary_name.as_deref(), Some("Sanderson, Brandon"));
    let (source_ids, source_names, canonical_id, canonical_name) = merge_payload(s);
    assert_eq!(source_ids, [duplicate]);
    assert_eq!(source_names, ["Sanderson, Brandon".to_string()]);
    assert_eq!(canonical_id, canonical);
    assert_eq!(canonical_name, "Brandon Sanderson");
}

#[tokio::test]
async fn detect_authors_sets_secondary_name_none_for_a_three_way_merge_group() {
    // #1855: a merge group of 3+ has no single "other" name, so
    // secondary_name must be None even though the two-way case above sets
    // it to the sole source name.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_author(&pool, "Brandon Sanderson", None).await;
    let dup_a = insert_author(&pool, "Sanderson, Brandon", None).await;
    // `authors.name` is UNIQUE COLLATE NOCASE, so the third duplicate must
    // differ from the other two by more than case — trailing punctuation
    // folds away in `normalize_author` without changing the DB-level name.
    let dup_b = insert_author(&pool, "Brandon Sanderson!!!", None).await;
    book_each_author(&pool, lib, &[canonical, dup_a, dup_b]).await;

    let suggestions = detect_authors(&pool).await.unwrap();
    let merges: Vec<_> = suggestions
        .iter()
        .filter(|s| s.action == CleanupAction::Merge)
        .collect();
    assert_eq!(merges.len(), 1);
    let s = merges[0];
    assert_eq!(s.secondary_name, None);
    let (source_ids, source_names, canonical_id, _) = merge_payload(s);
    // All three rows have one linked book, so the tie breaks toward the
    // lowest id — the first one inserted.
    assert_eq!(canonical_id, canonical);
    assert_eq!(source_ids.len(), 2);
    assert!(source_ids.contains(&dup_a));
    assert!(source_ids.contains(&dup_b));
    assert_eq!(source_names.len(), 2);
}

#[tokio::test]
async fn detect_authors_surfaces_tier1_fuzzy_merge_with_visible_score_at_or_above_085() {
    // AC2: token-set Jaccard >= 0.85 surfaces with a visible score even
    // though the two names never share a Tier-0 normalized key.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let shorter = insert_author(&pool, "Alpha Bravo Charlie Delta Echo Foxtrot Golf", None).await;
    let longer = insert_author(
        &pool,
        "Alpha Bravo Charlie Delta Echo Foxtrot Golf Hotel",
        None,
    )
    .await;
    book_each_author(&pool, lib, &[shorter, longer]).await;

    let suggestions = detect_authors(&pool).await.unwrap();
    let merges: Vec<_> = suggestions
        .iter()
        .filter(|s| s.action == CleanupAction::Merge)
        .collect();
    assert_eq!(merges.len(), 1);
    let s = merges[0];
    assert_eq!(s.tier, Tier::One);
    assert!(s.score >= FUZZY_JACCARD_THRESHOLD, "score was {}", s.score);
    // Both rows have one linked book; the lower id (inserted first) wins the tie.
    let (_, _, canonical_id, _) = merge_payload(s);
    assert_eq!(canonical_id, shorter);
}

#[tokio::test]
async fn detect_authors_emits_delete_suggestions_for_junk_author_patterns() {
    // AC3: junk-author regex emits Delete suggestions for known tooling
    // artifacts, and leaves a real author alone.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let calibre = insert_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]", None).await;
    let smashwords = insert_author(&pool, "Smashwords, Inc.", None).await;
    let real = insert_author(&pool, "Isaac Asimov", None).await;
    book_each_author(&pool, lib, &[calibre, smashwords, real]).await;

    let suggestions = detect_authors(&pool).await.unwrap();
    let deletes: Vec<_> = suggestions
        .iter()
        .filter(|s| s.action == CleanupAction::Delete)
        .collect();
    assert_eq!(deletes.len(), 2);
    for s in &deletes {
        assert_eq!(s.kind, CleanupKind::Author);
        assert_eq!(s.tier, Tier::Zero);
        assert_eq!(s.score, 1.0);
    }
    let deleted_ids: Vec<i64> = deletes
        .iter()
        .map(|s| match &s.payload {
            CleanupPayload::Delete { entity_id, .. } => *entity_id,
            other => panic!("expected a Delete payload, got {other:?}"),
        })
        .collect();
    assert!(deleted_ids.contains(&calibre));
    assert!(deleted_ids.contains(&smashwords));
}

#[tokio::test]
async fn detect_authors_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_authors(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}

// detect_series
#[tokio::test]
async fn detect_series_collapses_normalized_key_duplicates_into_one_tier0_merge() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_series(&pool, "The Stormlight Archive", None).await;
    let duplicate = insert_series(&pool, "Stormlight Archive, The", None).await;
    let b1 = insert_book(&pool, lib, "u1", "Book One").await;
    link_series(&pool, b1, canonical).await;
    let b2 = insert_book(&pool, lib, "u2", "Book Two").await;
    link_series(&pool, b2, duplicate).await;

    let suggestions = detect_series(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.kind, CleanupKind::Series);
    assert_eq!(s.action, CleanupAction::Merge);
    assert_eq!(s.tier, Tier::Zero);
    let (source_ids, _, canonical_id, _) = merge_payload(s);
    assert_eq!(source_ids, [duplicate]);
    assert_eq!(canonical_id, canonical);
}

#[tokio::test]
async fn detect_series_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_series(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}

// detect_tags_merge
#[tokio::test]
async fn detect_tags_merge_collapses_punctuation_variants_into_one_tier0_merge() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_tag(&pool, "Sci-Fi").await;
    let duplicate = insert_tag(&pool, "Sci Fi").await;
    book_each_tag(&pool, lib, &[canonical, duplicate]).await;

    let suggestions = detect_tags_merge(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.kind, CleanupKind::Tag);
    assert_eq!(s.action, CleanupAction::Merge);
    assert_eq!(s.tier, Tier::Zero);
    let (source_ids, _, canonical_id, _) = merge_payload(s);
    // Both rows have equal book counts, so the lower id wins the tie.
    assert_eq!(canonical_id, canonical);
    assert_eq!(source_ids, [duplicate]);
}

#[tokio::test]
async fn detect_tags_merge_book_count_is_distinct_not_summed_across_the_group() {
    // A book linked to *both* the canonical and the duplicate tag (common
    // for tags: a book often carries near-duplicate tag spellings at once)
    // must be counted once, not twice.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_tag(&pool, "Sci-Fi").await;
    let duplicate = insert_tag(&pool, "Sci Fi").await;
    let shared = insert_book(&pool, lib, "u1", "Shared Book").await;
    let only_canonical = insert_book(&pool, lib, "u2", "Canonical Only").await;
    link_tag(&pool, shared, canonical).await;
    link_tag(&pool, shared, duplicate).await;
    link_tag(&pool, only_canonical, canonical).await;

    let suggestions = detect_tags_merge(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    // Naively summing per-entity counts would give 3 (2 + 1); the correct
    // distinct count is 2 (`shared` and `only_canonical`).
    assert_eq!(suggestions[0].book_count, 2);
}

#[tokio::test]
async fn detect_tags_merge_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_tags_merge(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}

// detect_tags_split
#[tokio::test]
async fn detect_tags_split_emits_a_tier0_suggestion_for_semicolon_soup_with_book_count() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let tag = insert_tag(&pool, "Fantasy Romance; Fantasy New Adult; Fantasy").await;
    let b1 = insert_book(&pool, lib, "u1", "Book One").await;
    let b2 = insert_book(&pool, lib, "u2", "Book Two").await;
    link_tag(&pool, b1, tag).await;
    link_tag(&pool, b2, tag).await;
    insert_tag(&pool, "Fantasy").await; // too short to be a split candidate

    let suggestions = detect_tags_split(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.action, CleanupAction::Split);
    assert_eq!(s.tier, Tier::Zero);
    assert_eq!(s.score, 1.0);
    assert_eq!(s.book_count, 2);
    match &s.payload {
        CleanupPayload::Split {
            source_id,
            atoms,
            delimiter,
            ..
        } => {
            assert_eq!(*source_id, tag);
            assert_eq!(delimiter, ";");
            assert_eq!(
                atoms,
                &vec![
                    "Fantasy Romance".to_string(),
                    "Fantasy New Adult".to_string(),
                    "Fantasy".to_string()
                ]
            );
        }
        other => panic!("expected a Split payload, got {other:?}"),
    }
}

#[tokio::test]
async fn detect_tags_split_emits_a_tier1_suggestion_for_a_long_embedded_comma_tag() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let name = "Epic High Fantasy, Sword and Sorcery, Heroic Journey Fiction";
    let tag = insert_tag(&pool, name).await;
    book_each_tag(&pool, lib, &[tag]).await;

    let suggestions = detect_tags_split(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.tier, Tier::One);
    assert_eq!(s.score, 0.75);
    match &s.payload {
        CleanupPayload::Split { delimiter, .. } => assert_eq!(delimiter, ","),
        other => panic!("expected a Split payload, got {other:?}"),
    }
}

#[tokio::test]
async fn detect_tags_split_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_tags_split(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}

// detect_book_titles
#[tokio::test]
async fn detect_book_titles_strips_an_author_prefix_as_a_tier0_rename() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let cruft = insert_book(
        &pool,
        lib,
        "u1",
        "Maas, Sarah J - A Court of Thorns and Roses",
    )
    .await;
    insert_book(&pool, lib, "u2", "Elantris").await; // clean, no suggestion

    let suggestions = detect_book_titles(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.kind, CleanupKind::BookTitle);
    assert_eq!(s.action, CleanupAction::Rename);
    assert_eq!(s.tier, Tier::Zero);
    assert_eq!(s.book_count, 1);
    assert_eq!(
        s.secondary_name.as_deref(),
        Some("A Court of Thorns and Roses")
    );
    match &s.payload {
        CleanupPayload::Rename { book_id, .. } => assert_eq!(*book_id, cruft),
        other => panic!("expected a Rename payload, got {other:?}"),
    }
}

#[tokio::test]
async fn detect_book_titles_strips_a_trailing_parenthetical_as_a_tier1_rename() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    insert_book(&pool, lib, "u1", "The Final Empire (Mistborn, #1)").await;

    let suggestions = detect_book_titles(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.tier, Tier::One);
    assert_eq!(s.score, 0.75);
    assert_eq!(s.secondary_name.as_deref(), Some("The Final Empire"));
}

#[tokio::test]
async fn detect_book_titles_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_book_titles(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}

// detect_all
#[tokio::test]
async fn detect_all_dispatches_every_detector_and_concatenates_results() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let a1 = insert_author(&pool, "Brandon Sanderson", None).await;
    let a2 = insert_author(&pool, "Sanderson, Brandon", None).await;
    let tag = insert_tag(&pool, "Fantasy Romance; Fantasy New Adult").await;
    let book = insert_book(&pool, lib, "u1", "Maas, Sarah J - A Court of Mist and Fury").await;
    link_author(&pool, book, a1).await;
    link_author(&pool, book, a2).await;
    link_tag(&pool, book, tag).await;

    let suggestions = detect_all(&pool).await.unwrap();
    let kinds: std::collections::HashSet<CleanupKind> =
        suggestions.iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&CleanupKind::Author));
    assert!(kinds.contains(&CleanupKind::Tag));
    assert!(kinds.contains(&CleanupKind::BookTitle));
}

#[tokio::test]
async fn detect_all_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_all(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}
