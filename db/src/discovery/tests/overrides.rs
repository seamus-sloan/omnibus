//! Metadata overrides on the discovery pages: an override that names an
//! author or series adds the book (case-insensitively, alongside canonical
//! members), one that clears them removes it, and an index-only override
//! reorders a canonical member.

use omnibus_shared::{Contributor, MetadataOverrides};

use super::super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::test_support::{author_id_by_name, seed_discovery_fixture, series_id_by_name};

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
