//! Unit tests for the author/series discovery-page queries: book
//! ordering, series-id population, and missing-id handling.

use omnibus_shared::{Contributor, MetadataOverrides};

use super::*;
use crate::author_photos_data::{upsert_author_photo, AuthorPhotoSource};
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{
    author_id_by_name, indexed, seed_books_for_one_author_and_series, seed_discovery_fixture,
    series_id_by_name, CoversTempDir,
};

#[tokio::test]
async fn get_author_returns_author_with_all_books_ordered_by_series_index() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let author = get_author(&pool, id).await.unwrap().expect("author exists");

    assert_eq!(author.name, "Ada Lovelace");
    assert_eq!(author.book_count, 3);
    assert_eq!(author.books.len(), 3);

    // Series books come first, ordered by series_index ASC (NULLS LAST
    // means the standalone trails).
    let titles: Vec<_> = author
        .books
        .iter()
        .filter_map(|b| b.title.clone())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Saga: Book One".to_string(),
            "Saga: Book Two".to_string(),
            "Standalone".to_string(),
        ]
    );
}
#[tokio::test]
async fn get_author_populates_series_id_on_books() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;
    let expected_sid = series_id_by_name(&pool, "Saga").await;

    let author = get_author(&pool, id).await.unwrap().unwrap();
    for book in author.books.iter().filter(|b| b.series.is_some()) {
        assert_eq!(
            book.series_id,
            Some(expected_sid),
            "series book should carry series_id"
        );
    }
    let standalone = author
        .books
        .iter()
        .find(|b| b.series.is_none())
        .expect("standalone present");
    assert_eq!(standalone.series_id, None);
}
#[tokio::test]
async fn get_author_returns_none_for_missing_id() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let missing = get_author(&pool, 999_999).await.unwrap();
    assert!(missing.is_none());
}
/// Add one book by Ada Lovelace under a second scan root, so the fixture
/// has an author whose books straddle two libraries.
async fn seed_second_library_book(pool: &sqlx::SqlitePool) {
    replace_books(
        pool,
        "/other",
        vec![indexed(
            "elsewhere.epub",
            Some("Elsewhere"),
            &["Ada Lovelace"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn get_author_for_paths_excludes_books_indexed_under_another_library() {
    let (pool, _guard) = seed_discovery_fixture().await;
    seed_second_library_book(&pool).await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let scoped = get_author_for_paths(&pool, id, &["/lib"])
        .await
        .unwrap()
        .expect("author exists");

    let titles: Vec<_> = scoped
        .books
        .iter()
        .filter_map(|b| b.title.clone())
        .collect();
    assert!(!titles.contains(&"Elsewhere".to_string()));
    assert_eq!(scoped.books.len(), 3);
    assert_eq!(scoped.book_count, 3, "book_count follows the same scope");
}

#[tokio::test]
async fn get_author_for_paths_includes_books_from_every_listed_library() {
    let (pool, _guard) = seed_discovery_fixture().await;
    seed_second_library_book(&pool).await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let scoped = get_author_for_paths(&pool, id, &["/lib", "/other"])
        .await
        .unwrap()
        .expect("author exists");

    assert_eq!(scoped.books.len(), 4);
    assert_eq!(scoped.book_count, 4);
}

#[tokio::test]
async fn get_author_for_paths_returns_no_books_for_an_empty_path_list() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let scoped = get_author_for_paths(&pool, id, &[])
        .await
        .unwrap()
        .expect("author exists");

    assert!(scoped.books.is_empty());
    assert_eq!(scoped.book_count, 0);
}

#[tokio::test]
async fn get_author_returns_books_from_every_library_when_unscoped() {
    let (pool, _guard) = seed_discovery_fixture().await;
    seed_second_library_book(&pool).await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let author = get_author(&pool, id).await.unwrap().expect("author exists");

    assert_eq!(author.books.len(), 4);
    assert_eq!(author.book_count, 4);
}

#[tokio::test]
async fn get_author_for_paths_returns_none_for_missing_id() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let missing = get_author_for_paths(&pool, 999_999, &["/lib"])
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn get_author_for_paths_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_author_for_paths(&pool, 1, &["/lib"]).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Db(_)));
}

#[tokio::test]
async fn get_series_returns_books_ordered_by_series_index() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = series_id_by_name(&pool, "Saga").await;

    let series = get_series(&pool, id).await.unwrap().expect("series exists");
    assert_eq!(series.name, "Saga");
    assert_eq!(series.book_count, 2);

    let titles: Vec<_> = series
        .books
        .iter()
        .filter_map(|b| b.title.clone())
        .collect();
    assert_eq!(
        titles,
        vec!["Saga: Book One".to_string(), "Saga: Book Two".to_string()]
    );
    // Each book should carry the parent series id back out so the
    // frontend can navigate cross-references without an extra lookup.
    for book in &series.books {
        assert_eq!(book.series_id, Some(id));
    }
}
#[tokio::test]
async fn get_series_returns_none_for_missing_id() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let missing = get_series(&pool, 999_999).await.unwrap();
    assert!(missing.is_none());
}
#[tokio::test]
async fn get_author_caps_books_at_max_discovery_books() {
    let _covers = CoversTempDir::new("author_cap");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = MAX_DISCOVERY_BOOKS + 25;
    let (author_id, _series_id) = seed_books_for_one_author_and_series(&pool, total).await;

    let author = get_author(&pool, author_id)
        .await
        .unwrap()
        .expect("author exists");
    assert_eq!(
        author.books.len() as i64,
        MAX_DISCOVERY_BOOKS,
        "get_author must cap the nested books vec at MAX_DISCOVERY_BOOKS"
    );
    assert_eq!(
        author.book_count as i64, total,
        "book_count must report the true (uncapped) shelf size"
    );
    assert!(
        author.book_count > author.books.len(),
        "truncation must be detectable as book_count > books.len()"
    );
}
#[tokio::test]
async fn get_series_caps_books_at_max_discovery_books() {
    let _covers = CoversTempDir::new("series_cap");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = MAX_DISCOVERY_BOOKS + 25;
    let (_author_id, series_id) = seed_books_for_one_author_and_series(&pool, total).await;

    let series = get_series(&pool, series_id)
        .await
        .unwrap()
        .expect("series exists");
    assert_eq!(
        series.books.len() as i64,
        MAX_DISCOVERY_BOOKS,
        "get_series must cap the nested books vec at MAX_DISCOVERY_BOOKS"
    );
    assert_eq!(
        series.book_count as i64, total,
        "book_count must report the true (uncapped) series size"
    );
    assert!(
        series.book_count > series.books.len(),
        "truncation must be detectable as book_count > books.len()"
    );
}
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
async fn get_author_includes_books_whose_override_names_this_author() {
    // Repro of the bug where renaming a book's author via the
    // metadata form (e.g. "Sanderson, Brandon" → "Brandon Sanderson")
    // left the book invisible on the new author's `/author/:id` page.
    // The override path writes JSON only — `books_authors_link` keeps
    // pointing at the canonical author row — so `get_author` must
    // layer overrides on top of the relational link at read time.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Set up the "Brandon Sanderson" vs "Sanderson, Brandon" shape:
    // one canonical author and a second name the user prefers, then
    // override one book to use the preferred name.
    let canonical_id = author_id_by_name(&pool, "Ada Lovelace").await;
    let preferred_id =
        sqlx::query_scalar::<_, i64>("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
            .bind("Lovelace, Ada")
            .bind("Lovelace, Ada")
            .fetch_one(&pool)
            .await
            .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let saga_one_id = saga_one.id;

    // saga1.epub canonically lists ["Ada Lovelace", "Grace Hopper"];
    // the override renames the primary author to "Lovelace, Ada".
    let ov = MetadataOverrides {
        creators: Some(vec![
            Contributor {
                name: "Lovelace, Ada".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            },
            Contributor {
                name: "Grace Hopper".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            },
        ]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    // Visiting the preferred-name author page must now include the
    // overridden book, even though `books_authors_link` for that book
    // still points at the canonical "Ada Lovelace" row.
    let preferred = get_author(&pool, preferred_id)
        .await
        .unwrap()
        .expect("author exists");
    let titles: Vec<_> = preferred
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        titles,
        vec!["Saga: Book One".to_string()],
        "override-named author must surface the book on /author/:id",
    );

    // And the canonical-name author page must drop it, because the
    // override replaced the creator list wholesale.
    let canonical = get_author(&pool, canonical_id)
        .await
        .unwrap()
        .expect("author exists");
    let canonical_titles: Vec<_> = canonical
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert!(
        !canonical_titles.contains(&"Saga: Book One".to_string()),
        "override moved the book off the canonical author, got {canonical_titles:?}",
    );

    // The card on the preferred-name page should show the override
    // creator name, not the canonical one.
    let card = &preferred.books[0];
    assert_eq!(card.id, saga_one_id);
    assert_eq!(
        card.creators.first().map(|c| c.name.as_str()),
        Some("Lovelace, Ada")
    );
}
#[tokio::test]
async fn get_author_returns_both_canonical_and_override_members_under_reanchored_predicate() {
    // F6 re-anchor guard: the author-detail predicate now drives through
    // `books_authors_link WHERE author = ?` (arm 1) UNION the
    // override-creators set (arm 2). For an author with BOTH a canonical
    // book and a book overridden INTO them, the detail page must return
    // the union of both — same books as the old `FROM books` scan.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // Niklaus canonically has only "Other Story" (Pioneers). Override the
    // standalone book (canonically Ada's) so its single creator becomes
    // Niklaus, pulling it into Niklaus's effective set via arm (2).
    let niklaus_id = author_id_by_name(&pool, "Niklaus Wirth").await;
    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Niklaus Wirth".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let niklaus = get_author(&pool, niklaus_id)
        .await
        .unwrap()
        .expect("author exists");
    assert_eq!(
        niklaus.book_count, 2,
        "canonical (Other Story) + override-in (Standalone) = 2",
    );
    let mut titles: Vec<_> = niklaus
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    titles.sort();
    assert_eq!(
        titles,
        vec!["Other Story".to_string(), "Standalone".to_string()],
        "detail predicate must union the canonical and override members",
    );
}
#[tokio::test]
async fn get_author_excludes_books_whose_override_clears_authors() {
    // A book whose override sets creators to the empty array should
    // disappear from every author's page, matching what the book
    // detail page already shows (no breadcrumb author).
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        creators: Some(vec![]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let ada = get_author(&pool, ada_id)
        .await
        .unwrap()
        .expect("author exists");
    let titles: Vec<_> = ada
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert!(
        !titles.contains(&"Standalone".to_string()),
        "override-cleared creators must drop the book from /author/:id, got {titles:?}",
    );
}
#[tokio::test]
async fn get_author_override_creator_match_is_case_insensitive() {
    // `authors.name` is `UNIQUE COLLATE NOCASE`, so an override that
    // differs only by case from the target author's row must still
    // surface the book on `/author/:id`. The override comparison
    // gets an explicit `COLLATE NOCASE` because the LHS is a
    // `json_extract(...)` expression (BINARY by default) and the RHS
    // is a bound parameter (also no collation).
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    // Override uses lowercase casing; canonical row is "Ada Lovelace".
    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "ada lovelace".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let ada = get_author(&pool, ada_id)
        .await
        .unwrap()
        .expect("author exists");
    let titles: Vec<_> = ada
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert!(
        titles.contains(&"Standalone".to_string()),
        "lowercase override should still match NOCASE author row, got {titles:?}",
    );
}
#[tokio::test]
async fn get_series_includes_books_added_via_override() {
    // Repro of the bug where editing a book to set its series via the
    // metadata form left the book invisible on `/series/:id`. The
    // override path only writes JSON into `metadata_overrides` and
    // never touches `books_series_link`, so `get_series` must layer
    // overrides on top of the relational link at read time.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let saga_id = series_id_by_name(&pool, "Saga").await;

    // Loner has no canonical series at all. After the override it
    // should show up as #3 in Saga, after the two indexed books.
    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let standalone_uuid = standalone.unique_identifier.clone().unwrap();
    let standalone_id = standalone.id;

    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some("3".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &standalone_uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("series exists");
    assert_eq!(series.book_count, 3);

    let titles: Vec<_> = series
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Saga: Book One".to_string(),
            "Saga: Book Two".to_string(),
            "Standalone".to_string(),
        ],
        "override-set series_index=3 should sort the overridden book last",
    );

    // The overridden book must carry the parent series id so the card
    // links back to /series/:id.
    let overridden = series.books.iter().find(|b| b.id == standalone_id).unwrap();
    assert_eq!(overridden.series_id, Some(saga_id));
    assert_eq!(overridden.series.as_deref(), Some("Saga"));
}
#[tokio::test]
async fn get_series_reorders_canonical_member_via_index_only_override() {
    // Issue #154 guard: a `series_index` override on a book that is
    // *already canonically* in this series (no `series` override) must
    // still drive ordering. The pre-#154 `effective` CTE computed the
    // index independently of the name; the single-pass UNION rewrite
    // must preserve that — otherwise repositioning a book you didn't
    // move silently no-ops on the series page.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let saga_id = series_id_by_name(&pool, "Saga").await;

    // "Saga: Book One" is canonically index 1, "Book Two" index 2.
    // Override Book One's index to 5 (no series change) so it now
    // trails Book Two.
    let books = list_books(&pool, "/lib").await.unwrap();
    let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = book_one.unique_identifier.clone().unwrap();
    let ov = MetadataOverrides {
        series_index: Some("5".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("series exists");
    assert_eq!(
        series.book_count, 2,
        "membership is unchanged by an index-only override"
    );
    let titles: Vec<_> = series
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        titles,
        vec!["Saga: Book Two".to_string(), "Saga: Book One".to_string()],
        "index-only override on a canonical member must re-sort it",
    );
}
#[tokio::test]
async fn get_series_excludes_books_whose_override_clears_series() {
    // A book canonically in Saga whose override clears the series (sets
    // series to an empty string) should disappear from /series/:id,
    // matching what the book detail page already shows.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let saga_id = series_id_by_name(&pool, "Saga").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let book_two = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
    let uuid = book_two.unique_identifier.clone().unwrap();

    let ov = MetadataOverrides {
        series: Some(String::new()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("series exists");
    assert_eq!(series.book_count, 1);
    assert_eq!(
        series.books[0].title.as_deref(),
        Some("Saga: Book One"),
        "the unaffected book stays; the cleared one drops out",
    );
}
#[tokio::test]
async fn get_series_override_match_is_case_insensitive() {
    // The CTE's `series_name` column is BINARY by default — without
    // `COLLATE NOCASE` on the filter, an override that differs only
    // by case from the canonical series row fails to match, even
    // though `series.name` is `UNIQUE COLLATE NOCASE`.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let saga_id = series_id_by_name(&pool, "Saga").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    // Override uses lowercase casing; canonical row is "Saga".
    let ov = MetadataOverrides {
        series: Some("saga".into()),
        series_index: Some("3".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("series exists");
    let titles: Vec<_> = series
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert!(
        titles.contains(&"Standalone".to_string()),
        "lowercase override should still match NOCASE series row, got {titles:?}",
    );
}
#[tokio::test]
async fn get_author_empty_string_series_index_sorts_last() {
    // Mirror of get_series: clearing the position field (`Some("")`)
    // used to CAST('') to 0.0 in get_author's ORDER BY and sort the
    // book to the front of the author's shelf. NULLIF drops it to NULL
    // so NULLS LAST trails it behind positioned books.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let author_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = book_one.unique_identifier.clone().unwrap();

    // Keep Book One (canonical Saga #1) but clear its position.
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some(String::new()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let author = get_author(&pool, author_id)
        .await
        .unwrap()
        .expect("author exists");
    let titles: Vec<_> = author
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    let pos = |t: &str| titles.iter().position(|x| x == t).unwrap();
    assert!(
        pos("Saga: Book Two") < pos("Saga: Book One"),
        "cleared series_index should trail the positioned book, got {titles:?}",
    );
    assert_ne!(
        titles.first().map(String::as_str),
        Some("Saga: Book One"),
        "cleared series_index must not sort to the front, got {titles:?}",
    );
}
#[tokio::test]
async fn get_series_empty_string_series_index_sorts_last() {
    // `Some("")` from the edit form (user cleared the position
    // field) was sorting to the front because `CAST('' AS REAL)`
    // returns 0.0. NULLIF on the override value drops it to NULL,
    // and ORDER BY ... NULLS LAST trails it after positioned books.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let saga_id = series_id_by_name(&pool, "Saga").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    // Add Standalone to Saga but clear its position.
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some(String::new()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("series exists");
    let titles: Vec<_> = series
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Saga: Book One".to_string(),
            "Saga: Book Two".to_string(),
            "Standalone".to_string(),
        ],
        "empty-string series_index should trail positioned books, not lead them",
    );
}
#[tokio::test]
async fn get_series_pins_series_id_for_books_moved_between_series() {
    // A book canonically in Series A overridden into Series B used
    // to come back from get_series(B) with `series_id = Some(A)`
    // (BOOK_COLUMNS reads only books_series_link), so the card on
    // B's page would link back to /series/A. The fix pins
    // series_id/series unconditionally to the requested parent.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let pioneers_id = series_id_by_name(&pool, "Pioneers").await;

    // "Other Story" is canonically in Pioneers; override moves it
    // into Saga. Verify that opening Saga's page returns the book
    // pinned to Saga's id, not Pioneers'.
    let books = list_books(&pool, "/lib").await.unwrap();
    let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
    let uuid = other.unique_identifier.clone().unwrap();

    let saga_id = series_id_by_name(&pool, "Saga").await;
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some("5".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let saga = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("Saga exists");
    let moved = saga
        .books
        .iter()
        .find(|b| b.title.as_deref() == Some("Other Story"))
        .expect("override moved Other Story into Saga");
    assert_eq!(
        moved.series_id,
        Some(saga_id),
        "card on Saga's page must link back to Saga, not the canonical Pioneers",
    );
    assert_eq!(moved.series.as_deref(), Some("Saga"));

    // And it should be gone from Pioneers' page.
    let pioneers = get_series(&pool, pioneers_id)
        .await
        .unwrap()
        .expect("Pioneers exists");
    assert!(
        !pioneers
            .books
            .iter()
            .any(|b| b.title.as_deref() == Some("Other Story")),
        "override moved Other Story off Pioneers",
    );
}
#[tokio::test]
async fn get_author_populates_has_photo() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    // No row → false.
    let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
    assert!(!ada.has_photo, "no row should yield has_photo = false");

    // Letter marker → still false (negative-cache shouldn't render an img).
    upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();
    let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
    assert!(
        !ada.has_photo,
        "letter marker should yield has_photo = false"
    );

    // Manual upload → true.
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
    let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
    assert!(ada.has_photo, "manual upload should yield has_photo = true");
}

#[tokio::test]
async fn get_tag_cloud_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_tag_cloud(&pool).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Db(_)));
}

// --- genre cloud --------------------------------------------------------

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
