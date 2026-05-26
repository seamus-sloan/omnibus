//! Inline integration tests for the db query layer. Lives here as a
//! single file (rather than co-located per module) because so many tests
//! exercise behavior that spans two or three of the new modules
//! (covers + sync, settings + cover cleanup, overrides + book reads).
//! Splitting them per module would force every test helper out into
//! `pub(crate)` and obscure what's actually under test. Future tests for
//! a single module's internal behavior should still land in that
//! module's inline `#[cfg(test)] mod tests`.

#[allow(unused_imports, dead_code)]
mod tests {
    use crate::author_photos_data::{
        author_photo_status, delete_author, delete_author_photo, get_author_photo,
        upsert_author_photo, AuthorPhotoSource,
    };
    use crate::books::{
        count_books, get_book, library_from_db, library_from_db_with_total, list_books,
        list_indexed_rows, sanitize_description, search_books, MAX_BOOKS_RETURNED,
    };
    use crate::browse::{list_authors, list_series};
    use crate::covers::test_helpers::CoversTempDir;
    use crate::covers::{cover_path_for, delete_cover_files_for, get_cover, write_cover_file};
    use crate::discovery::test_helpers::*;
    use crate::discovery::{get_author, get_series, get_tag_cloud, MAX_DISCOVERY_BOOKS};
    use crate::ebook::IndexedBook;
    use crate::helpers::stable_uuid;
    use crate::metadata_overrides::{
        delete_metadata_overrides, get_metadata_overrides, merge_metadata_overrides,
        upsert_metadata_overrides, write_override_cover,
    };
    use crate::palette::search_palette;
    use crate::pool::init_db;
    use crate::settings::{last_indexed_at, prune_orphan_libraries, set_settings, Settings};
    use crate::sync::test_helpers::{indexed, indexed_with_stat};
    use crate::sync::{replace_books, sync_books, SyncPlan};
    use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};
    use sqlx::{Row, SqlitePool};

    // -----------------------------------------------------------------
    // F1.12 index pages — list_authors / list_series
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn list_authors_returns_all_with_counts_and_alpha_order() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let authors = list_authors(&pool, "/lib").await.unwrap();

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
        let authors = list_authors(&pool, "/no-such-library").await.unwrap();
        assert!(
            authors.is_empty(),
            "unknown library path must yield empty list"
        );
    }

    #[tokio::test]
    async fn list_authors_returns_empty_for_empty_library() {
        let _guard = CoversTempDir::new("empty_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let authors = list_authors(&pool, "/lib").await.unwrap();
        assert!(authors.is_empty());
    }

    #[tokio::test]
    async fn list_series_returns_all_with_counts_and_alpha_order() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/lib").await.unwrap();

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
        let series = list_series(&pool, "/lib").await.unwrap();

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
        let series = list_series(&pool, "/no-such-library").await.unwrap();
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

        let authors = list_authors(&pool, "/lib").await.unwrap();
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

        let authors = list_authors(&pool, "/lib").await.unwrap();
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

        let series = list_series(&pool, "/lib").await.unwrap();
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

        let series = list_series(&pool, "/lib").await.unwrap();
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

    // ── search_palette ──────────────────────────────────────────────

    #[tokio::test]
    async fn palette_books_match_title() {
        let _covers = CoversTempDir::new("palette_books");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["Horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["Horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "dracula").await.unwrap();
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Dracula");
        assert_eq!(results.books[0].author_display, "Bram Stoker");
        assert!(results.books[0].formats.contains(&"EPUB".to_string()));
    }

    #[tokio::test]
    async fn palette_authors_match_substring() {
        let _covers = CoversTempDir::new("palette_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Babel"),
                &["R. F. Kuang"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "kuang").await.unwrap();
        assert!(!results.authors.is_empty(), "should match author substring");
        assert_eq!(results.authors[0].name, "R. F. Kuang");
        assert_eq!(results.authors[0].book_count, 1);
    }

    #[tokio::test]
    async fn palette_series_match() {
        let _covers = CoversTempDir::new("palette_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Book Two"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "poppy").await.unwrap();
        assert!(!results.series.is_empty(), "should match series substring");
        assert_eq!(results.series[0].name, "Poppy War");
        assert_eq!(results.series[0].book_count, 2);
    }

    #[tokio::test]
    async fn palette_tags_match() {
        let _covers = CoversTempDir::new("palette_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "academia").await.unwrap();
        assert!(!results.tags.is_empty(), "should match tag substring");
        assert_eq!(results.tags[0].name, "Dark academia");
        assert_eq!(results.tags[0].book_count, 1);
    }

    /// Bug #1 (display side): the palette must show the overridden title,
    /// not the canonical scanned `b.title`, so what the user clicks matches
    /// what they searched for.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_title() {
        let _covers = CoversTempDir::new("palette_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Edited Title".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(palette.books[0].title, "Edited Title");
    }

    /// Bug #1 (display side): overriding the creators list rebuilds the
    /// comma-joined `author_display` so the palette subtitle matches the
    /// detail page.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_author_display() {
        let _covers = CoversTempDir::new("palette_override_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Searchable"),
                &["Original Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                creators: Some(vec![
                    Contributor {
                        name: "First Override".into(),
                        ..Default::default()
                    },
                    Contributor {
                        name: "Second Override".into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Searchable").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].author_display,
            "First Override, Second Override"
        );
    }

    /// Palette book hits should surface a user-uploaded cover even when the
    /// scanned book had no cover. Mirrors `apply_overrides` so the palette
    /// row doesn't go cover-less for an override-only cover.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_cover() {
        let _covers = CoversTempDir::new("palette_override_cover");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Indexed book with no scanned cover.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Coverless Searchable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let book = list_books(&pool, "/lib").await.unwrap().remove(0);
        let uuid = book.unique_identifier.clone().unwrap();

        // Set has_cover_override = true with no text edits.
        upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Coverless").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].cover_url,
            Some(format!("/api/covers/{uuid}"))
        );
    }

    // #128: lock the wiring between the palette and `build_fts_match`'s
    // facet prefixes. A regression in the facet parser could otherwise
    // silently break palette tag:/author:/series: queries without any
    // palette test failing.

    #[tokio::test]
    async fn palette_book_matches_tag_facet() {
        let _covers = CoversTempDir::new("palette_tag_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["vampires"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["monsters"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "tag:vampires").await.unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "tag:vampires should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "tag:vampires should not match Frankenstein"
        );
    }

    #[tokio::test]
    async fn palette_book_matches_author_facet() {
        let _covers = CoversTempDir::new("palette_author_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author:stoker")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "author:stoker should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "author:stoker should not match Frankenstein"
        );
    }

    #[tokio::test]
    async fn palette_book_matches_series_facet() {
        let _covers = CoversTempDir::new("palette_series_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author A"],
                    &[],
                    Some(("Dracula Chronicles", "1")),
                    None,
                ),
                indexed("b.epub", Some("Unrelated"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "series:dracula")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Book One"),
            "series:dracula should match Book One, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Unrelated"),
            "series:dracula should not match Unrelated"
        );
    }

    #[tokio::test]
    async fn palette_empty_query_returns_empty() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let results = search_palette(&pool, "/lib", "   ").await.unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
        assert_eq!(results.query, "");
    }

    #[tokio::test]
    async fn palette_no_results() {
        let _covers = CoversTempDir::new("palette_no_results");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "zzzznonexistent")
            .await
            .unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
    }

    #[tokio::test]
    async fn palette_scoped_to_library() {
        let _covers = CoversTempDir::new("palette_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Alpha"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b.epub",
                Some("Beta"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib-a", "tolkien").await.unwrap();
        // Books should only include lib-a
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Alpha");
        // Author book_count should be 1 (scoped to lib-a), not 2
        assert_eq!(results.authors[0].book_count, 1);
    }

    #[tokio::test]
    async fn palette_duration_populated() {
        let _covers = CoversTempDir::new("palette_duration");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author").await.unwrap();
        // duration_ms should be populated (at least 0 — we just check it's set)
        assert!(results.duration_ms < 10000, "duration should be reasonable");
    }

    /// #127 regression coverage: after collapsing the correlated
    /// `book_count` / `EXISTS` subqueries into a single JOIN+GROUP BY,
    /// `l.path = ?1` must still be applied **before** the aggregate so the
    /// scoped library's count doesn't pick up rows from sibling libraries.
    /// This exercises all three taxonomies (authors, series, tags) plus
    /// ordering — the seeded set has 3 matching books in /lib-a and 2 in
    /// /lib-b for the same author/series/tag, and the rare "Sole" author
    /// only appears in /lib-b, so it must be absent from /lib-a results.
    #[tokio::test]
    async fn palette_taxonomy_counts_scoped_per_library() {
        let _covers = CoversTempDir::new("palette_taxonomy_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed(
                    "a1.epub",
                    Some("Alpha One"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "a2.epub",
                    Some("Alpha Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
                indexed(
                    "a3.epub",
                    Some("Alpha Three"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "3")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![
                indexed(
                    "b1.epub",
                    Some("Beta One"),
                    &["Shared Author", "Sole Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "b2.epub",
                    Some("Beta Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Scope to /lib-a — author/series/tag counts must be 3, not 5.
        let results = search_palette(&pool, "/lib-a", "Shared").await.unwrap();

        let author = results
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-a results");
        assert_eq!(
            author.book_count, 3,
            "author count must be scoped to /lib-a, got {results:?}"
        );
        assert!(
            !results.authors.iter().any(|a| a.name == "Sole Author"),
            "Sole Author lives only in /lib-b and must not appear"
        );

        let series = results
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-a results");
        assert_eq!(
            series.book_count, 3,
            "series count must be scoped to /lib-a"
        );

        let tag = results
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-a results");
        assert_eq!(tag.book_count, 3, "tag count must be scoped to /lib-a");

        // Cross-check /lib-b counts to make sure the same query returns 2.
        let results_b = search_palette(&pool, "/lib-b", "Shared").await.unwrap();
        let author_b = results_b
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-b results");
        assert_eq!(author_b.book_count, 2);
        let series_b = results_b
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-b results");
        assert_eq!(series_b.book_count, 2);
        let tag_b = results_b
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-b results");
        assert_eq!(tag_b.book_count, 2);
    }

    #[tokio::test]
    async fn palette_author_count_reflects_overrides() {
        // F5.1: the palette author count must match the merged
        // (override-aware) view, not the raw `books_authors_link` count.
        // Repro of the "Sanderson, Brandon still says 4 books" report:
        // every canonical book for an author was reassigned to a
        // differently-named author through the metadata edit form, so the
        // palette must report 0 books for the source name and the full
        // count for the destination name.
        let _covers = CoversTempDir::new("palette_author_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Two books canonically by "Last, First", plus one book by the
        // already-correct "First Last" so the destination author has a
        // canonical anchor (palette visibility requires ≥1 canonical link).
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Last, First"], &[], None, None),
                indexed("b.epub", Some("B"), &["Last, First"], &[], None, None),
                indexed("c.epub", Some("C"), &["First Last"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // User edits a.epub and b.epub through the metadata form to
        // rename their author to "First Last" — overrides only, no
        // change to the relational link table.
        let books = list_books(&pool, "/lib").await.unwrap();
        for filename in ["a.epub", "b.epub"] {
            let book = books.iter().find(|b| b.filename == filename).unwrap();
            let uuid = book.unique_identifier.clone().unwrap();
            let ov = MetadataOverrides {
                creators: Some(vec![Contributor {
                    name: "First Last".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                }]),
                ..Default::default()
            };
            upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
                .await
                .unwrap();
        }

        let results = search_palette(&pool, "/lib", "Last").await.unwrap();

        // Source author still visible (canonical anchor remains), but
        // count must reflect the effective view: 0 books.
        let source = results
            .authors
            .iter()
            .find(|a| a.name == "Last, First")
            .expect("source author still appears in palette");
        assert_eq!(
            source.book_count, 0,
            "renamed-away author must report effective count 0, got {results:?}",
        );

        // Destination author picks up the override-renamed books on top
        // of its own canonical anchor: 1 + 2 = 3.
        let dest = results
            .authors
            .iter()
            .find(|a| a.name == "First Last")
            .expect("destination author present");
        assert_eq!(
            dest.book_count, 3,
            "destination author must include override-renamed books, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_tag_count_reflects_overrides() {
        // F5.1: same shape for tags. `overrides.subjects` replaces the
        // canonical tag list wholesale, so a book moved between tags
        // must shift both counts.
        let _covers = CoversTempDir::new("palette_tag_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["tag-source"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["tag-source"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["tag-dest"], None, None),
            ],
        )
        .await
        .unwrap();

        // Move a.epub off tag-source and onto tag-dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["tag-dest".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "tag-").await.unwrap();
        let source = results
            .tags
            .iter()
            .find(|t| t.name == "tag-source")
            .expect("tag-source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "tag-source should drop a.epub after override, got {results:?}",
        );
        let dest = results
            .tags
            .iter()
            .find(|t| t.name == "tag-dest")
            .expect("tag-dest present");
        assert_eq!(
            dest.book_count, 2,
            "tag-dest should add the override-tagged a.epub, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_series_count_reflects_overrides() {
        // F5.1: same shape as palette_author_count_reflects_overrides
        // but for the series tile. Books moved into a series via
        // `overrides.series` must add to the destination count; books
        // moved out drop from the source count.
        let _covers = CoversTempDir::new("palette_series_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["X"],
                    &[],
                    Some(("Series Source", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["X"],
                    &[],
                    Some(("Series Source", "2")),
                    None,
                ),
                indexed(
                    "c.epub",
                    Some("C"),
                    &["X"],
                    &[],
                    Some(("Series Dest", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Move a.epub from Series Source to Series Dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            series: Some("Series Dest".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // "Series" matches both names.
        let results = search_palette(&pool, "/lib", "Series").await.unwrap();
        let source = results
            .series
            .iter()
            .find(|s| s.name == "Series Source")
            .expect("Series Source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "Series Source should count only b.epub after a.epub is overridden away, got {results:?}",
        );
        let dest = results
            .series
            .iter()
            .find(|s| s.name == "Series Dest")
            .expect("Series Dest present");
        assert_eq!(
            dest.book_count, 2,
            "Series Dest should count its canonical c.epub plus the override-moved a.epub, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_series_author_display_reflects_override() {
        // F5.1: the "by X" line on a series tile must follow the first
        // book's effective creator, not the canonical one — otherwise
        // renaming the author through the metadata edit form leaves the
        // palette showing the old name.
        let _covers = CoversTempDir::new("palette_series_author_display");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "k1.epub",
                Some("K1"),
                &["Old Name"],
                &[],
                Some(("Kingsway", "1")),
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "New Name".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "Kingsway").await.unwrap();
        let kingsway = results
            .series
            .iter()
            .find(|s| s.name == "Kingsway")
            .expect("Kingsway present");
        assert_eq!(
            kingsway.author_display.as_deref(),
            Some("New Name"),
            "palette author line must follow override.creators, got {results:?}",
        );
    }

    /// #127: capture `EXPLAIN QUERY PLAN` for each of the three rewritten
    /// taxonomy queries and assert the planner uses the link-table indexes.
    /// This is a structural check — it doesn't pin the literal plan string
    /// (SQLite's wording can shift across point releases) but it does fail
    /// loudly if any of the link tables fall back to a full SCAN, which
    /// would defeat the whole point of this optimization.
    #[tokio::test]
    async fn palette_taxonomy_query_plans_use_indexes() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        async fn plan_text(pool: &SqlitePool, sql: &str) -> String {
            let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
                .bind("/lib")
                .bind("%x%")
                .bind(5_i32)
                .fetch_all(pool)
                .await
                .unwrap();
            rows.iter()
                .map(|r| r.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        // Authors — override-aware count, must still drive through the
        // covering `books_authors_link` index when checking visibility
        // and when the no-override branch of the CASE runs.
        let plan = plan_text(
            &pool,
            "SELECT a.id, a.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.creators') IS NOT NULL \
                          THEN EXISTS (SELECT 1 FROM json_each(mo.overrides, '$.creators') je \
                                        WHERE json_extract(je.value, '$.name') = a.name) \
                        ELSE EXISTS (SELECT 1 FROM books_authors_link bal \
                                      WHERE bal.book = b.id AND bal.author = a.id) \
                      END \
              ) AS book_count \
             FROM authors a \
             WHERE a.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_authors_link bal \
                             JOIN books b ON b.id = bal.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bal.author = a.id AND l.path = ?1) \
             ORDER BY book_count DESC, a.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_authors_link") && !plan.contains("SCAN bal"),
            "authors plan should not full-scan the link table:\n{plan}"
        );

        // Series — override-aware count + author_display, must still drive
        // through the `books_series_link` index for visibility and the
        // no-override branch of the CASE.
        let plan = plan_text(
            &pool,
            "SELECT s.id, s.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.series') IS NOT NULL \
                          THEN json_extract(mo.overrides, '$.series') = s.name \
                        ELSE EXISTS (SELECT 1 FROM books_series_link bsl \
                                      WHERE bsl.book = b.id AND bsl.series = s.id) \
                      END \
              ) AS book_count \
             FROM series s \
             WHERE s.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_series_link bsl \
                             JOIN books b ON b.id = bsl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bsl.series = s.id AND l.path = ?1) \
             ORDER BY book_count DESC, s.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_series_link") && !plan.contains("SCAN bsl"),
            "series plan should not full-scan the link table:\n{plan}"
        );

        // Tags — override-aware count, must still drive through the
        // `books_tags_link` index for visibility and the no-override
        // branch of the CASE.
        let plan = plan_text(
            &pool,
            "SELECT t.id, t.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.subjects') IS NOT NULL \
                          THEN EXISTS (SELECT 1 FROM json_each(mo.overrides, '$.subjects') je \
                                        WHERE je.value = t.name) \
                        ELSE EXISTS (SELECT 1 FROM books_tags_link btl \
                                      WHERE btl.book = b.id AND btl.tag = t.id) \
                      END \
              ) AS book_count \
             FROM tags t \
             WHERE t.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_tags_link btl \
                             JOIN books b ON b.id = btl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE btl.tag = t.id AND l.path = ?1) \
             ORDER BY book_count DESC, t.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_tags_link") && !plan.contains("SCAN btl"),
            "tags plan should not full-scan the link table:\n{plan}"
        );
    }

    // -------------------------------------------------------------------------
    // F1.11 author profile photo tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn author_photo_roundtrips_manual_upload() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let bytes = b"\xFF\xD8\xFFfake-jpeg".to_vec();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(&bytes),
        )
        .await
        .unwrap();

        let (mime, fetched) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(fetched, bytes);
    }

    #[tokio::test]
    async fn author_photo_letter_marker_returns_none() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();

        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }

    #[tokio::test]
    async fn author_photo_status_none_when_unset() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;
        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn author_photo_upsert_replaces_existing_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // Letter marker first, then a manual upload replaces it.
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/png"),
            Some(b"\x89PNG\r\n\x1a\nfake"),
        )
        .await
        .unwrap();

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Manual);
        let (mime, _) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/png");
    }

    #[tokio::test]
    async fn author_photo_delete_clears_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfoo"),
        )
        .await
        .unwrap();
        delete_author_photo(&pool, ada_id).await.unwrap();

        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
    }

    /// `list_authors` mirrors the `get_author` has_photo semantics so the
    /// /authors index can pick the right avatar without a per-card detail
    /// fetch. Same three-state matrix: no row → false, `letter` marker →
    /// false, `manual` upload → true.
    #[tokio::test]
    async fn list_authors_populates_has_photo() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let initial = list_authors(&pool, "/lib").await.unwrap();
        let ada = initial.iter().find(|a| a.id == ada_id).unwrap();
        assert!(!ada.has_photo, "no row should yield has_photo = false");

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let after_letter = list_authors(&pool, "/lib").await.unwrap();
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
        let after_upload = list_authors(&pool, "/lib").await.unwrap();
        let ada = after_upload.iter().find(|a| a.id == ada_id).unwrap();
        assert!(ada.has_photo, "manual upload should yield has_photo = true");
    }

    // -------------------------------------------------------------------------
    // ignored_authors blocklist (F5.9-lite reindex-resurrection guard)
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // delete_author (F5.9-lite admin "Delete author" primitive)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn delete_author_removes_links_and_inserts_blocklist_row() {
        let _covers = CoversTempDir::new("delete_author_basic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Junk Author"], &[], None, None),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["Junk Author", "Real Author"],
                    &[],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind("Junk Author")
            .fetch_one(&pool)
            .await
            .unwrap();

        let unlinked = delete_author(&pool, junk_id).await.unwrap();
        assert_eq!(unlinked, 2, "both books should report as un-linked");

        let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE id = ?")
            .bind(junk_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(junk_count, 0, "authors row should be gone");

        let link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE author = ?")
                .bind(junk_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            link_count, 0,
            "no link rows should remain for deleted author"
        );

        let blocklist_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
                .bind("Junk Author")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(blocklist_count, 1, "name must be added to ignored_authors");

        // Real Author on book B should still be linked.
        let books = list_books(&pool, "/lib").await.unwrap();
        let b = books
            .iter()
            .find(|x| x.title.as_deref() == Some("B"))
            .unwrap();
        let creators: Vec<&str> = b.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }

    #[tokio::test]
    async fn delete_author_is_no_op_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let unlinked = delete_author(&pool, 99_999).await.unwrap();
        assert_eq!(unlinked, 0);
        let blocklist_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            blocklist_count, 0,
            "missing id must not leak a ghost blocklist row"
        );
    }

    #[tokio::test]
    async fn delete_author_survives_reindex() {
        // Full durability check: delete the junk author, then run the
        // reindex pipeline against a fixture that *still* lists the junk
        // contributor in its OPF (simulated via replace_books with the
        // same input). The blocklist row inserted by delete_author must
        // keep resolve_or_insert_author from re-creating the author.
        let _covers = CoversTempDir::new("delete_author_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();

        let junk_name = "calibre (8.0.0) [https://calibre-ebook.com]";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        delete_author(&pool, junk_id).await.unwrap();

        // Simulated reindex: same OPF contents, run through the
        // indexing pipeline again. Without the blocklist guard this
        // would re-create the junk author and relink it.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            junk_after, 0,
            "second reindex must not resurrect a deleted author"
        );

        let books = list_books(&pool, "/lib").await.unwrap();
        let creators: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }
}
