//! The tag and genre clouds: counts ordered by count then name, the empty
//! cases, override subjects counted without double-counting or duplicates
//! and matched case-insensitively, genres folding case variants and
//! dropping once unnamed, the DB-failure paths, and the error conversion.

use omnibus_shared::MetadataOverrides;

use super::super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, seed_discovery_fixture, CoversTempDir};

#[tokio::test]
async fn get_tag_cloud_returns_counts_ordered_by_count_then_name() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let tags = get_tag_cloud(&pool).await.unwrap();

    // Fixture has: fiction × 2, classic × 1, essay × 1, nonfiction × 1.
    // Order: cnt DESC, then name ASC.
    let names: Vec<_> = tags.iter().map(|t| t.name.clone()).collect();
    assert_eq!(
        names,
        vec![
            "fiction".to_string(),
            "classic".to_string(),
            "essay".to_string(),
            "nonfiction".to_string(),
        ]
    );
    assert_eq!(tags[0].count, 2);
    assert!(tags[1..].iter().all(|t| t.count == 1));
}

#[tokio::test]
async fn get_tag_cloud_returns_empty_vec_when_no_tags() {
    let _guard = CoversTempDir::new("empty_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // No books, no tags.
    let tags = get_tag_cloud(&pool).await.unwrap();
    assert!(tags.is_empty());
}

#[tokio::test]
async fn get_tag_cloud_counts_reflect_overrides() {
    // Per-tag counts follow the merged (override-aware) membership,
    // not the canonical link rows.
    let _guard = CoversTempDir::new("tag_cloud_overrides");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
            indexed("b.epub", Some("B"), &["X"], &["fiction"], None, None),
            indexed("c.epub", Some("C"), &["X"], &["essay"], None, None),
        ],
    )
    .await
    .unwrap();

    // Sanity: canonical counts before any overrides.
    let pre = get_tag_cloud(&pool).await.unwrap();
    let fiction_pre = pre
        .iter()
        .find(|t| t.name == "fiction")
        .expect("fiction present pre-override");
    assert_eq!(fiction_pre.count, 2);

    // Reassign a.epub: drop "fiction", add "essay".
    let books = list_books(&pool, "/lib").await.unwrap();
    let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
    let uuid = a.unique_identifier.clone().unwrap();
    let ov = MetadataOverrides {
        subjects: Some(vec!["essay".into()]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let post = get_tag_cloud(&pool).await.unwrap();
    let fiction = post
        .iter()
        .find(|t| t.name == "fiction")
        .expect("fiction still visible (canonical anchor remains on b.epub)");
    assert_eq!(
        fiction.count, 1,
        "fiction should drop a.epub after override, got {post:?}",
    );
    let essay = post
        .iter()
        .find(|t| t.name == "essay")
        .expect("essay present");
    assert_eq!(
        essay.count, 2,
        "essay should pick up override-tagged a.epub, got {post:?}",
    );
}

#[tokio::test]
async fn get_tag_cloud_counts_canonical_and_override_subjects_without_double_count() {
    // Regression guard for the single-pass GROUP BY rewrite: one tag
    // ("essay") must sum exactly one canonical member and one
    // override-only member without double-counting either. Arm 1
    // (canonical link, `tag_name = NULL`) and arm 2 (override subject,
    // `tag_id = NULL`) are disjoint per book, so the OR-join must add the
    // two distinct books to cnt=2 — not 1 (under-count) and not 3+
    // (double-count via the OR predicate).
    let _guard = CoversTempDir::new("tag_cloud_no_double_count");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![
            // a.epub: canonically "essay".
            indexed("a.epub", Some("A"), &["X"], &["essay"], None, None),
            // b.epub: canonically "fiction"; will be overridden to "essay".
            indexed("b.epub", Some("B"), &["X"], &["fiction"], None, None),
        ],
    )
    .await
    .unwrap();

    // Override b.epub to "essay" so it reaches the tag via arm 2 only,
    // while a.epub reaches it via arm 1 only.
    let books = list_books(&pool, "/lib").await.unwrap();
    let b = books.iter().find(|x| x.filename == "b.epub").unwrap();
    let uuid = b.unique_identifier.clone().unwrap();
    let ov = MetadataOverrides {
        subjects: Some(vec!["essay".into()]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let tags = get_tag_cloud(&pool).await.unwrap();
    let essay = tags
        .iter()
        .find(|t| t.name == "essay")
        .expect("essay present");
    assert_eq!(
        essay.count, 2,
        "essay must sum the canonical and override members exactly once each, got {tags:?}",
    );
    // "fiction" loses its only member to the override; with zero effective
    // books it drops out of the cloud entirely. Its `tags` row and canonical
    // link survive underneath (revert-to-scanned needs them) — hidden, not
    // deleted.
    assert!(
        !tags.iter().any(|t| t.name == "fiction"),
        "a fully-overridden-away tag must be hidden from the cloud, got {tags:?}",
    );
    let fiction_row: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE name = 'fiction'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        fiction_row, 1,
        "the canonical tags row must survive for revert-to-scanned"
    );
}

#[tokio::test]
async fn get_tag_cloud_dedupes_duplicate_subject_strings_within_one_override() {
    // The UNION (not UNION ALL) in arm (2) collapses a `["essay","essay"]`
    // override to a single effective row, so the GROUP BY pass counts the
    // book once. A naive UNION ALL rewrite would double-count it.
    let _guard = CoversTempDir::new("tag_cloud_dedupe_override");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &["essay"], None, None)],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let a = books.iter().find(|x| x.filename == "a.epub").unwrap();
    let uuid = a.unique_identifier.clone().unwrap();
    let ov = MetadataOverrides {
        subjects: Some(vec!["essay".into(), "essay".into()]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let tags = get_tag_cloud(&pool).await.unwrap();
    let essay = tags
        .iter()
        .find(|t| t.name == "essay")
        .expect("essay present");
    assert_eq!(
        essay.count, 1,
        "duplicate subject strings in one override must count the book once, got {tags:?}",
    );
}

#[tokio::test]
async fn get_tag_cloud_matches_override_subjects_case_insensitively() {
    // `tags.name` dedupes NOCASE, so a case-variant override ("fiction"
    // typed against a canonical "Fiction" row) materializes no new row —
    // the membership match must fold case too, or the override member is
    // silently dropped from the canonical row's count.
    let _guard = CoversTempDir::new("tag_cloud_case_fold");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &["Fiction"], None, None),
            indexed("b.epub", Some("B"), &["X"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let b = books.iter().find(|x| x.filename == "b.epub").unwrap();
    let uuid = b.unique_identifier.clone().unwrap();
    let ov = MetadataOverrides {
        subjects: Some(vec!["fiction".into()]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let tags = get_tag_cloud(&pool).await.unwrap();
    let fiction = tags
        .iter()
        .find(|t| t.name == "Fiction")
        .expect("canonical Fiction row present");
    assert_eq!(
        fiction.count, 2,
        "case-variant override subject must count toward the NOCASE-unique row, got {tags:?}",
    );
    assert!(
        !tags.iter().any(|t| t.name == "fiction"),
        "no separate lowercase row may surface, got {tags:?}",
    );
}

#[tokio::test]
async fn get_tag_cloud_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_tag_cloud(&pool).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Db(_)));
}

/// Seed `count` books under `/lib` and return their uuids in filename order.
async fn seed_books_for_genres(pool: &sqlx::SqlitePool, count: usize) -> Vec<String> {
    let books: Vec<_> = (0..count)
        .map(|i| {
            let name = format!("{}.epub", (b'a' + u8::try_from(i).unwrap()) as char);
            indexed(&name, Some(&format!("Book {i}")), &["X"], &[], None, None)
        })
        .collect();
    replace_books(pool, "/lib", books).await.unwrap();
    let mut listed = list_books(pool, "/lib").await.unwrap();
    listed.sort_by(|a, b| a.filename.cmp(&b.filename));
    listed
        .into_iter()
        .map(|b| b.unique_identifier.unwrap())
        .collect()
}

async fn set_genres(pool: &sqlx::SqlitePool, uuid: &str, genres: &[&str], user_id: i64) {
    let ov = MetadataOverrides {
        genres: Some(genres.iter().map(|g| (*g).to_string()).collect()),
        ..Default::default()
    };
    upsert_metadata_overrides(pool, uuid, &ov, false, user_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn get_genre_cloud_returns_counts_ordered_by_count_then_name() {
    let _guard = CoversTempDir::new("genre_cloud_order");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuids = seed_books_for_genres(&pool, 3).await;

    set_genres(&pool, &uuids[0], &["Horror", "Mystery"], user_id).await;
    set_genres(&pool, &uuids[1], &["Horror"], user_id).await;
    set_genres(&pool, &uuids[2], &["Adventure"], user_id).await;

    let genres = get_genre_cloud(&pool).await.unwrap();
    let pairs: Vec<(&str, usize)> = genres.iter().map(|g| (g.name.as_str(), g.count)).collect();
    assert_eq!(
        pairs,
        vec![("Horror", 2), ("Adventure", 1), ("Mystery", 1)],
        "count desc, then name asc"
    );
}

#[tokio::test]
async fn get_genre_cloud_returns_empty_vec_when_no_book_has_genres() {
    let _guard = CoversTempDir::new("genre_cloud_empty");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Tags on every book, genres on none — the two vocabularies are separate.
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["X"],
            &["fiction"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    assert!(get_genre_cloud(&pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn get_genre_cloud_drops_a_genre_once_its_last_book_stops_naming_it() {
    let _guard = CoversTempDir::new("genre_cloud_drop");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuids = seed_books_for_genres(&pool, 1).await;

    set_genres(&pool, &uuids[0], &["Western"], user_id).await;
    assert_eq!(get_genre_cloud(&pool).await.unwrap().len(), 1);

    // Replacing the list orphans "Western"; the write path reaps its row.
    set_genres(&pool, &uuids[0], &["Noir"], user_id).await;
    let genres = get_genre_cloud(&pool).await.unwrap();
    assert_eq!(genres.len(), 1);
    assert_eq!(genres[0].name, "Noir");
}

#[tokio::test]
async fn get_genre_cloud_folds_case_variants_into_the_canonical_row() {
    let _guard = CoversTempDir::new("genre_cloud_case");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuids = seed_books_for_genres(&pool, 2).await;

    set_genres(&pool, &uuids[0], &["Sci-Fi"], user_id).await;
    set_genres(&pool, &uuids[1], &["sci-fi"], user_id).await;

    let genres = get_genre_cloud(&pool).await.unwrap();
    assert_eq!(genres.len(), 1, "NOCASE-unique `genres` row, one entry");
    assert_eq!(genres[0].name, "Sci-Fi", "first spelling coined the row");
    assert_eq!(genres[0].count, 2);
}

#[tokio::test]
async fn get_genre_cloud_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_genre_cloud(&pool).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Db(_)));
}

#[test]
fn discovery_error_from_metadata_overrides_error_returns_other_for_bulk_write_variants() {
    let err = DiscoveryError::from(
        crate::metadata_overrides::MetadataOverridesError::BookNotFound("abc".into()),
    );
    assert!(
        matches!(&err, DiscoveryError::Other(msg) if msg.contains("abc")),
        "expected Other carrying the source message, got {err:?}"
    );
}
