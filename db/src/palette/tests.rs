//! Unit tests for the search-palette query layer: per-arm matching,
//! library scoping, override-aware counts, and `EXPLAIN QUERY PLAN`
//! guards against full-scan regressions on the link tables.

use omnibus_shared::{Contributor, MetadataOverrides};
use sqlx::{Row, SqlitePool};

use super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

// ── search_palette ───────────────────────────────────────────────
#[tokio::test]
async fn search_palette_books_match_title() {
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
async fn search_palette_authors_match_substring() {
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
    // F1 results page: the "incl. <title>" line draws on the author's first
    // book in the library.
    assert_eq!(
        results.authors[0].lead_book_title.as_deref(),
        Some("Babel"),
        "author hit should carry its lead book title"
    );
}
#[tokio::test]
async fn search_palette_series_match() {
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
    // F1 results page: the "incl. <title>" line draws on the first book in
    // the series by sort order.
    assert_eq!(
        results.series[0].lead_book_title.as_deref(),
        Some("Book One"),
        "series hit should carry its lead book title"
    );
}
#[tokio::test]
async fn search_palette_tags_match() {
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
async fn search_palette_book_hit_uses_overridden_title() {
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
async fn search_palette_book_hit_uses_overridden_author_display() {
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
async fn search_palette_book_hit_uses_overridden_cover() {
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
async fn search_palette_book_matches_tag_facet() {
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
async fn search_palette_book_matches_author_facet() {
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
async fn search_palette_book_matches_series_facet() {
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
async fn search_palette_empty_query_returns_empty() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let results = search_palette(&pool, "/lib", "   ").await.unwrap();
    assert!(results.books.is_empty());
    assert!(results.authors.is_empty());
    assert!(results.series.is_empty());
    assert!(results.tags.is_empty());
    assert_eq!(results.query, "");
}
#[tokio::test]
async fn search_palette_no_results() {
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
async fn search_palette_scoped_to_library() {
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
async fn search_palette_duration_populated() {
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
/// Regression coverage: after collapsing the correlated `book_count` /
/// `EXISTS` subqueries into a single JOIN+GROUP BY, `l.path = ?1` must
/// still be applied **before** the aggregate so the
/// scoped library's count doesn't pick up rows from sibling libraries.
/// This exercises all three taxonomies (authors, series, tags) plus
/// ordering — the seeded set has 3 matching books in /lib-a and 2 in
/// /lib-b for the same author/series/tag, and the rare "Sole" author
/// only appears in /lib-b, so it must be absent from /lib-a results.
#[tokio::test]
async fn search_palette_taxonomy_counts_scoped_per_library() {
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
async fn search_palette_author_count_reflects_overrides() {
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
async fn search_palette_tag_count_reflects_overrides() {
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
async fn search_palette_series_count_reflects_overrides() {
    // F5.1: same shape as search_palette_author_count_reflects_overrides
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
async fn search_palette_series_author_display_reflects_override() {
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
/// Capture `EXPLAIN QUERY PLAN` for each of the three rewritten taxonomy
/// queries and assert the planner uses the link-table indexes. This is a
/// structural check — it doesn't pin the literal plan string
/// (SQLite's wording can shift across point releases) but it does fail
/// loudly if any of the link tables fall back to a full SCAN, which
/// would defeat the whole point of this optimization.
#[tokio::test]
async fn search_palette_taxonomy_query_plans_use_indexes() {
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

    // Authors — single-pass effective-membership CTE (issue #154).
    // Canonical arm (1) of the union must still drive through the
    // `books_authors_link` index (the library-scoped join), not a full
    // scan, and the visibility `EXISTS` must too.
    let plan = plan_text(
        &pool,
        "WITH effective AS ( \
           SELECT bal.author AS author_id, NULL AS author_name, bal.book AS book_id \
             FROM books_authors_link bal \
             JOIN books b ON b.id = bal.book \
             JOIN scan_roots l2 ON l2.id = b.library_id \
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
            WHERE l2.path = ?1 \
              AND (mo.book_uuid IS NULL OR json_type(mo.overrides, '$.creators') IS NULL) \
           UNION \
           SELECT NULL AS author_id, json_extract(je.value, '$.name') AS author_name, b.id AS book_id \
             FROM books b \
             JOIN scan_roots l2 ON l2.id = b.library_id \
             JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
             JOIN json_each(mo.overrides, '$.creators') je \
            WHERE l2.path = ?1 AND json_type(mo.overrides, '$.creators') IS NOT NULL \
         ) \
         SELECT a.id, a.name, \
           (SELECT COUNT(*) FROM effective e \
             WHERE e.author_id = a.id OR e.author_name = a.name) AS book_count \
         FROM authors a \
         WHERE a.name LIKE ?2 ESCAPE '\\' \
           AND EXISTS (SELECT 1 FROM books_authors_link bal \
                         JOIN books b ON b.id = bal.book \
                         JOIN scan_roots l ON l.id = b.library_id \
                        WHERE bal.author = a.id AND l.path = ?1) \
         ORDER BY book_count DESC, a.name \
         LIMIT ?3",
    )
    .await;
    assert!(
        !plan.contains("SCAN books_authors_link") && !plan.contains("SCAN bal"),
        "authors plan should not full-scan the link table:\n{plan}"
    );

    // Series — single-pass effective-membership CTE (issue #154).
    // Canonical arm (1) and the visibility `EXISTS` must still drive
    // through the `books_series_link` index, not a full scan.
    let plan = plan_text(
        &pool,
        "WITH effective AS ( \
           SELECT bsl.series AS series_id, NULL AS series_name, bsl.book AS book_id \
             FROM books_series_link bsl \
             JOIN books b ON b.id = bsl.book \
             JOIN scan_roots l2 ON l2.id = b.library_id \
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
            WHERE l2.path = ?1 \
              AND (mo.book_uuid IS NULL OR json_type(mo.overrides, '$.series') IS NULL) \
           UNION \
           SELECT NULL AS series_id, json_extract(mo.overrides, '$.series') AS series_name, b.id AS book_id \
             FROM books b \
             JOIN scan_roots l2 ON l2.id = b.library_id \
             JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
            WHERE l2.path = ?1 AND json_type(mo.overrides, '$.series') IS NOT NULL \
         ) \
         SELECT s.id, s.name, \
           (SELECT COUNT(*) FROM effective e \
             WHERE e.series_id = s.id OR e.series_name = s.name) AS book_count \
         FROM series s \
         WHERE s.name LIKE ?2 ESCAPE '\\' \
           AND EXISTS (SELECT 1 FROM books_series_link bsl \
                         JOIN books b ON b.id = bsl.book \
                         JOIN scan_roots l ON l.id = b.library_id \
                        WHERE bsl.series = s.id AND l.path = ?1) \
         ORDER BY book_count DESC, s.name \
         LIMIT ?3",
    )
    .await;
    assert!(
        !plan.contains("SCAN books_series_link") && !plan.contains("SCAN bsl"),
        "series plan should not full-scan the link table:\n{plan}"
    );

    // Tags — single-pass effective-membership CTE (issue #154).
    // Canonical arm (1) and the visibility `EXISTS` must still drive
    // through the `books_tags_link` index, not a full scan.
    let plan = plan_text(
        &pool,
        "WITH effective AS ( \
           SELECT btl.tag AS tag_id, NULL AS tag_name, btl.book AS book_id \
             FROM books_tags_link btl \
             JOIN books b ON b.id = btl.book \
             JOIN scan_roots l2 ON l2.id = b.library_id \
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
            WHERE l2.path = ?1 \
              AND (mo.book_uuid IS NULL OR json_type(mo.overrides, '$.subjects') IS NULL) \
           UNION \
           SELECT NULL AS tag_id, je.value AS tag_name, b.id AS book_id \
             FROM books b \
             JOIN scan_roots l2 ON l2.id = b.library_id \
             JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
             JOIN json_each(mo.overrides, '$.subjects') je \
            WHERE l2.path = ?1 AND json_type(mo.overrides, '$.subjects') IS NOT NULL \
         ) \
         SELECT t.id, t.name, \
           (SELECT COUNT(*) FROM effective e \
             WHERE e.tag_id = t.id OR e.tag_name = t.name) AS book_count \
         FROM tags t \
         WHERE t.name LIKE ?2 ESCAPE '\\' \
           AND EXISTS (SELECT 1 FROM books_tags_link btl \
                         JOIN books b ON b.id = btl.book \
                         JOIN scan_roots l ON l.id = b.library_id \
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
/// F1 results-page header: per-category totals report the true match count
/// even when the 5-hit display cap clips the returned vec. Seven books share
/// a title token, one author, and one tag — so the books arm caps at 5 hits
/// but `book_total` is 7, while `author_total`/`tag_total` (under the cap)
/// equal their vec lengths. `total_count()` sums the true totals.
#[tokio::test]
async fn search_palette_totals_report_uncapped_counts() {
    let _covers = CoversTempDir::new("palette_totals");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let books: Vec<_> = (0..7)
        .map(|i| {
            indexed(
                &format!("q{i}.epub"),
                Some(&format!("Quest {i}")),
                &["Quest Author"],
                &["questing"],
                None,
                None,
            )
        })
        .collect();
    replace_books(&pool, "/lib", books).await.unwrap();

    let results = search_palette(&pool, "/lib", "quest").await.unwrap();

    assert_eq!(results.books.len(), 5, "books arm caps display at 5");
    assert_eq!(
        results.book_total, 7,
        "book_total is the uncapped match count"
    );
    assert_eq!(results.author_total, 1, "one matching author");
    assert_eq!(results.authors.len(), 1);
    assert_eq!(results.tag_total, 1, "one matching tag");
    assert_eq!(results.series_total, 0, "no series seeded");
    assert_eq!(
        results.total_count(),
        7 + 1 + 1,
        "books + authors + tags (no series)"
    );
}

// ── per-arm helpers (direct coverage) ────────────────────────────
// The arm functions below are normally reached only through
// `search_palette`; these exercise them directly with an already-escaped
// `%pattern%` (the caller escapes LIKE metacharacters before calling).

#[tokio::test]
async fn search_authors_returns_matching_author_with_scoped_count() {
    let _covers = CoversTempDir::new("arm_search_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Babel"), &["R. F. Kuang"], &[], None, None),
            indexed(
                "b.epub",
                Some("Yellowface"),
                &["R. F. Kuang"],
                &[],
                None,
                None,
            ),
            indexed("c.epub", Some("Other"), &["Someone Else"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_authors(&pool, "/lib", "%kuang%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "only the Kuang author matches");
    assert_eq!(hits[0].name, "R. F. Kuang");
    assert_eq!(hits[0].book_count, 2);
    assert_eq!(hits[0].lead_book_title.as_deref(), Some("Babel"));
}

#[tokio::test]
async fn search_authors_respects_limit() {
    let _covers = CoversTempDir::new("arm_search_authors_limit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let books: Vec<_> = (0..4)
        .map(|i| {
            indexed(
                &format!("m{i}.epub"),
                Some(&format!("Book {i}")),
                &[&format!("Match Author {i}")],
                &[],
                None,
                None,
            )
        })
        .collect();
    replace_books(&pool, "/lib", books).await.unwrap();

    let hits = search_authors(&pool, "/lib", "%match%", 2).await.unwrap();
    assert_eq!(hits.len(), 2, "arm caps at the supplied limit");
}

#[tokio::test]
async fn count_authors_counts_visible_matches_uncapped() {
    let _covers = CoversTempDir::new("arm_count_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let books: Vec<_> = (0..7)
        .map(|i| {
            indexed(
                &format!("m{i}.epub"),
                Some(&format!("Book {i}")),
                &[&format!("Match Author {i}")],
                &[],
                None,
                None,
            )
        })
        .collect();
    replace_books(&pool, "/lib", books).await.unwrap();

    // Uncapped: counts all 7 matching authors regardless of the display cap.
    let total = count_authors(&pool, "/lib", "%match%").await.unwrap();
    assert_eq!(total, 7);
    // A non-matching pattern counts zero.
    assert_eq!(count_authors(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
}

#[tokio::test]
async fn count_authors_is_scoped_to_library() {
    let _covers = CoversTempDir::new("arm_count_authors_scoped");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib-a",
        vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib-b",
        vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
    )
    .await
    .unwrap();

    // Same author name in both libraries, but the count is per-library.
    assert_eq!(
        count_authors(&pool, "/lib-a", "%tolkien%").await.unwrap(),
        1
    );
}

#[tokio::test]
async fn search_series_returns_matching_series_with_author_display() {
    let _covers = CoversTempDir::new("arm_search_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Book One"),
                &["Author X"],
                &[],
                Some(("Poppy War", "1")),
                None,
            ),
            indexed(
                "b.epub",
                Some("Book Two"),
                &["Author X"],
                &[],
                Some(("Poppy War", "2")),
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let hits = search_series(&pool, "/lib", "%poppy%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Poppy War");
    assert_eq!(hits[0].book_count, 2);
    assert_eq!(hits[0].author_display.as_deref(), Some("Author X"));
    assert_eq!(hits[0].lead_book_title.as_deref(), Some("Book One"));
}

#[tokio::test]
async fn count_series_counts_visible_matches_scoped() {
    let _covers = CoversTempDir::new("arm_count_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("A"),
                &["X"],
                &[],
                Some(("Match Alpha", "1")),
                None,
            ),
            indexed(
                "b.epub",
                Some("B"),
                &["X"],
                &[],
                Some(("Match Beta", "1")),
                None,
            ),
            indexed(
                "c.epub",
                Some("C"),
                &["X"],
                &[],
                Some(("Unrelated", "1")),
                None,
            ),
        ],
    )
    .await
    .unwrap();

    assert_eq!(count_series(&pool, "/lib", "%match%").await.unwrap(), 2);
    assert_eq!(count_series(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
}

#[tokio::test]
async fn search_tags_returns_matching_tag_with_scoped_count() {
    let _covers = CoversTempDir::new("arm_search_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("A"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("B"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            ),
            indexed("c.epub", Some("C"), &["Author"], &["Cozy"], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_tags(&pool, "/lib", "%academia%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Dark academia");
    assert_eq!(hits[0].book_count, 2);
}

#[tokio::test]
async fn count_tags_counts_visible_matches_scoped() {
    let _covers = CoversTempDir::new("arm_count_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &["match-one"], None, None),
            indexed("b.epub", Some("B"), &["X"], &["match-two"], None, None),
            indexed("c.epub", Some("C"), &["X"], &["other"], None, None),
        ],
    )
    .await
    .unwrap();

    assert_eq!(count_tags(&pool, "/lib", "%match-%").await.unwrap(), 2);
    assert_eq!(count_tags(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
}

#[tokio::test]
async fn search_palette_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = search_palette(&pool, "/lib", "query").await.unwrap_err();
    assert!(matches!(err, PaletteError::Db(_)));
}
