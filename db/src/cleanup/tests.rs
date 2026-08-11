//! Tests for the library-cleanup detectors: pure unit coverage for the
//! normalized-key fold, Jaccard scoring, first-token blocking, and the
//! regex heuristics, plus pool-backed coverage of each `detect_*` entry
//! point's happy path, Tier-0/Tier-1 branches, and `CleanupError::Db`
//! propagation.

use sqlx::SqlitePool;

use super::*;
use crate::pool::init_db;

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

async fn seed_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_book(pool: &SqlitePool, lib_id: i64, uuid: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(format!("/lib/{uuid}"))
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_author(pool: &SqlitePool, name: &str, sort: Option<&str>) -> i64 {
    sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(sort)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_series(pool: &SqlitePool, name: &str, sort: Option<&str>) -> i64 {
    sqlx::query_scalar("INSERT INTO series (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(sort)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_tag(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO tags (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn link_author(pool: &SqlitePool, book_id: i64, author_id: i64) {
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book_id)
        .bind(author_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_series(pool: &SqlitePool, book_id: i64, series_id: i64) {
    sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?, ?)")
        .bind(book_id)
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_tag(pool: &SqlitePool, book_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(book_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
}

fn merge_payload(s: &DetectedSuggestion) -> (&[i64], &[String], i64, &str) {
    match &s.payload {
        CleanupPayload::Merge {
            source_ids,
            source_names,
            canonical_id,
            canonical_name,
        } => (source_ids, source_names, *canonical_id, canonical_name),
        other => panic!("expected a Merge payload, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Pure unit tests: tier0_key
// ---------------------------------------------------------------------------

#[test]
fn tier0_key_collapses_last_first_swap_the_same_via_sort_or_name() {
    let via_name = tier0_key("Sanderson, Brandon", None);
    let via_plain = tier0_key("Brandon Sanderson", None);
    let via_sort = tier0_key("junk display name", Some("Sanderson, Brandon"));
    assert_eq!(via_name, Some("brandon sanderson".to_string()));
    assert_eq!(via_name, via_plain);
    assert_eq!(via_name, via_sort, "sort must win over name when present");
}

#[test]
fn tier0_key_returns_none_when_nothing_survives() {
    assert_eq!(tier0_key("---", None), None);
}

// ---------------------------------------------------------------------------
// Pure unit tests: Jaccard + first-token blocking
// ---------------------------------------------------------------------------

#[test]
fn jaccard_scores_full_overlap_partial_overlap_and_disjoint_sets() {
    let a = token_set("alpha bravo charlie");
    let b = token_set("alpha bravo charlie");
    assert_eq!(jaccard(&a, &b), 1.0);

    let c = token_set("alpha bravo charlie delta echo foxtrot golf");
    let d = token_set("alpha bravo charlie delta echo foxtrot golf hotel");
    assert_eq!(jaccard(&c, &d), 7.0 / 8.0);

    let e = token_set("alpha bravo");
    let f = token_set("charlie delta");
    assert_eq!(jaccard(&e, &f), 0.0);
}

#[test]
fn fuzzy_merge_suggestions_blocks_pairs_with_different_first_token() {
    // Both keys share 12 of 14 total tokens (>= 0.85 Jaccard if compared),
    // but differ in their *first* token — first-token blocking must keep
    // them in separate buckets and never compare them.
    let key_a = "quebec charlie delta echo foxtrot golf hotel india juliet kilo lima mike november";
    let key_b = "romeo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november";
    assert!(jaccard(&token_set(key_a), &token_set(key_b)) >= FUZZY_JACCARD_THRESHOLD);

    let row_a = EntityRow {
        id: 1,
        name: "A".into(),
        sort: None,
        book_count: 0,
    };
    let row_b = EntityRow {
        id: 2,
        name: "B".into(),
        sort: None,
        book_count: 0,
    };
    let singles = [(key_a, &row_a), (key_b, &row_b)];
    assert!(fuzzy_merge_suggestions(CleanupKind::Author, &singles).is_empty());
}

// ---------------------------------------------------------------------------
// Pure unit tests: pick_canonical
// ---------------------------------------------------------------------------

#[test]
fn pick_canonical_prefers_more_books_then_lower_id() {
    let few_books = EntityRow {
        id: 5,
        name: "Few".into(),
        sort: None,
        book_count: 1,
    };
    let many_books = EntityRow {
        id: 9,
        name: "Many".into(),
        sort: None,
        book_count: 4,
    };
    let group = [&few_books, &many_books];
    assert_eq!(pick_canonical(&group).unwrap().id, many_books.id);

    let tie_high_id = EntityRow {
        id: 9,
        name: "High".into(),
        sort: None,
        book_count: 1,
    };
    let tie_low_id = EntityRow {
        id: 2,
        name: "Low".into(),
        sort: None,
        book_count: 1,
    };
    let tied = [&tie_high_id, &tie_low_id];
    assert_eq!(pick_canonical(&tied).unwrap().id, tie_low_id.id);
}

#[test]
fn pick_canonical_returns_none_for_an_empty_group() {
    let empty: [&EntityRow; 0] = [];
    assert!(pick_canonical(&empty).is_none());
}

// ---------------------------------------------------------------------------
// Pure unit tests: split_candidate
// ---------------------------------------------------------------------------

#[test]
fn split_candidate_detects_semicolon_soup_as_tier0() {
    let (tier, delimiter, atoms) =
        split_candidate("Fantasy Romance; Fantasy New Adult; Fantasy").unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(delimiter, ";");
    assert_eq!(
        atoms,
        vec!["Fantasy Romance", "Fantasy New Adult", "Fantasy"]
    );
}

#[test]
fn split_candidate_detects_long_embedded_comma_name_as_tier1() {
    let name = "Epic High Fantasy, Sword and Sorcery, Heroic Journey Fiction";
    assert!(name.len() >= TAG_SPLIT_LONG_LEN);
    let (tier, delimiter, atoms) = split_candidate(name).unwrap();
    assert_eq!(tier, Tier::One);
    assert_eq!(delimiter, ",");
    assert_eq!(
        atoms,
        vec![
            "Epic High Fantasy",
            "Sword and Sorcery",
            "Heroic Journey Fiction"
        ]
    );
}

#[test]
fn split_candidate_returns_none_for_a_short_plain_tag() {
    assert_eq!(split_candidate("Fantasy"), None);
    // A single short comma-pair isn't "long" enough for the tier-1 heuristic.
    assert_eq!(split_candidate("Sci-Fi, Fantasy"), None);
}

// ---------------------------------------------------------------------------
// Pure unit tests: strip_title_cruft
// ---------------------------------------------------------------------------

fn title_regexes() -> (Vec<Regex>, Regex) {
    (
        compile_all(TITLE_PREFIX_PATTERNS).unwrap(),
        Regex::new(TITLE_SUFFIX_PATTERN).unwrap(),
    )
}

#[test]
fn strip_title_cruft_strips_last_first_dash_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) = strip_title_cruft(
        "Maas, Sarah J - A Court of Thorns and Roses",
        &prefixes,
        &suffix,
    )
    .unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "A Court of Thorns and Roses");
}

#[test]
fn strip_title_cruft_strips_series_bracket_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) =
        strip_title_cruft("[Mistborn 01] The Final Empire", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_series_hash_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) =
        strip_title_cruft("Mistborn #1 The Final Empire", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_strips_acronym_index_dash_prefix_as_tier0() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) = strip_title_cruft("ToG04-Queen of Shadows", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::Zero);
    assert_eq!(proposed, "Queen of Shadows");
}

#[test]
fn strip_title_cruft_strips_trailing_parenthetical_as_tier1() {
    let (prefixes, suffix) = title_regexes();
    let (tier, proposed) =
        strip_title_cruft("The Final Empire (Mistborn, #1)", &prefixes, &suffix).unwrap();
    assert_eq!(tier, Tier::One);
    assert_eq!(proposed, "The Final Empire");
}

#[test]
fn strip_title_cruft_returns_none_for_a_clean_title() {
    let (prefixes, suffix) = title_regexes();
    assert_eq!(strip_title_cruft("Elantris", &prefixes, &suffix), None);
}

// ---------------------------------------------------------------------------
// Pure unit tests: pattern compilation
// ---------------------------------------------------------------------------

#[test]
fn compile_all_compiles_every_valid_pattern() {
    assert_eq!(compile_all(&["^a", "b$"]).unwrap().len(), 2);
}

#[test]
#[allow(clippy::invalid_regex)] // deliberately malformed input under test
fn compile_all_returns_err_for_an_invalid_pattern() {
    assert!(compile_all(&["valid", "("]).is_err());
}

#[test]
#[allow(clippy::invalid_regex)] // deliberately malformed input under test
fn cleanup_error_pattern_variant_wraps_an_invalid_regex_error() {
    let regex_err = Regex::new("(").unwrap_err();
    let err: CleanupError = regex_err.into();
    assert!(matches!(err, CleanupError::Pattern(_)));
}

// ---------------------------------------------------------------------------
// detect_authors
// ---------------------------------------------------------------------------

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
async fn detect_authors_surfaces_tier1_fuzzy_merge_with_visible_score_at_or_above_085() {
    // AC2: token-set Jaccard >= 0.85 surfaces with a visible score even
    // though the two names never share a Tier-0 normalized key.
    let pool = new_pool().await;
    let shorter = insert_author(&pool, "Alpha Bravo Charlie Delta Echo Foxtrot Golf", None).await;
    let _longer = insert_author(
        &pool,
        "Alpha Bravo Charlie Delta Echo Foxtrot Golf Hotel",
        None,
    )
    .await;

    let suggestions = detect_authors(&pool).await.unwrap();
    let merges: Vec<_> = suggestions
        .iter()
        .filter(|s| s.action == CleanupAction::Merge)
        .collect();
    assert_eq!(merges.len(), 1);
    let s = merges[0];
    assert_eq!(s.tier, Tier::One);
    assert!(s.score >= FUZZY_JACCARD_THRESHOLD, "score was {}", s.score);
    // Both rows have 0 linked books; the lower id (inserted first) wins the tie.
    let (_, _, canonical_id, _) = merge_payload(s);
    assert_eq!(canonical_id, shorter);
}

#[tokio::test]
async fn detect_authors_emits_delete_suggestions_for_junk_author_patterns() {
    // AC3: junk-author regex emits Delete suggestions for known tooling
    // artifacts, and leaves a real author alone.
    let pool = new_pool().await;
    let calibre = insert_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]", None).await;
    let smashwords = insert_author(&pool, "Smashwords, Inc.", None).await;
    insert_author(&pool, "Isaac Asimov", None).await;

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

// ---------------------------------------------------------------------------
// detect_series
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detect_series_collapses_normalized_key_duplicates_into_one_tier0_merge() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let canonical = insert_series(&pool, "The Stormlight Archive", None).await;
    let duplicate = insert_series(&pool, "Stormlight Archive, The", None).await;
    let b1 = insert_book(&pool, lib, "u1", "Book One").await;
    link_series(&pool, b1, canonical).await;

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

// ---------------------------------------------------------------------------
// detect_tags_merge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detect_tags_merge_collapses_punctuation_variants_into_one_tier0_merge() {
    let pool = new_pool().await;
    let canonical = insert_tag(&pool, "Sci-Fi").await;
    let duplicate = insert_tag(&pool, "Sci Fi").await;

    let suggestions = detect_tags_merge(&pool).await.unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s.kind, CleanupKind::Tag);
    assert_eq!(s.action, CleanupAction::Merge);
    assert_eq!(s.tier, Tier::Zero);
    let (source_ids, _, canonical_id, _) = merge_payload(s);
    // Both rows have equal (zero) book counts, so the lower id wins the tie.
    assert_eq!(canonical_id, canonical);
    assert_eq!(source_ids, [duplicate]);
}

#[tokio::test]
async fn detect_tags_merge_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = detect_tags_merge(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupError::Db(_)));
}

// ---------------------------------------------------------------------------
// detect_tags_split
// ---------------------------------------------------------------------------

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
    let name = "Epic High Fantasy, Sword and Sorcery, Heroic Journey Fiction";
    insert_tag(&pool, name).await;

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

// ---------------------------------------------------------------------------
// detect_book_titles
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// detect_all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detect_all_dispatches_every_detector_and_concatenates_results() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    insert_author(&pool, "Brandon Sanderson", None).await;
    insert_author(&pool, "Sanderson, Brandon", None).await;
    insert_tag(&pool, "Fantasy Romance; Fantasy New Adult").await;
    insert_book(&pool, lib, "u1", "Maas, Sarah J - A Court of Mist and Fury").await;

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
