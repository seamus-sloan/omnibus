//! The taxonomy count columns: per-library scoping across every arm, the
//! `EXPLAIN QUERY PLAN` guard against full scans on the link tables, and
//! the uncapped totals reported beside the capped rows.

use sqlx::{Row, SqlitePool};

use super::super::*;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

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

/// Capture `EXPLAIN QUERY PLAN` for each of the three taxonomy queries and
/// assert the link table is read **once** — one pass to build the
/// `effective` membership set, every other reference an indexed seek. That
/// is what the single-pass rewrite bought (issue #154); a second scan means
/// a per-entity correlated subquery has crept back in and the plan is
/// O(entities × link rows) again. Structural, not a pinned plan string:
/// SQLite's wording shifts across point releases.
///
/// The SQL comes from the arms themselves, not a copy — a copy stops
/// describing the query it guards the moment the real one changes, and this
/// one had already drifted (it still scoped on `l2.path = ?1`).
///
/// That one membership scan is newly a scan rather than a seek: visibility is
/// a disjunction — under a configured root *or* holding a physical copy — so
/// the planner can no longer drive in from `scan_roots`. Same O(link rows)
/// work, different order.
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

    for (arm, sql, link_table, link_alias) in [
        (
            "authors",
            authors::search_authors_sql(),
            "books_authors_link",
            "bal",
        ),
        (
            "series",
            series::search_series_sql(),
            "books_series_link",
            "bsl",
        ),
        ("tags", tags::search_tags_sql(), "books_tags_link", "btl"),
    ] {
        let plan = plan_text(&pool, sql).await;
        let mut scans = 0;
        for line in plan.lines() {
            let mut words = line.split_whitespace();
            let (Some(verb), Some(target)) = (words.next(), words.next()) else {
                continue;
            };
            // Whole-word match on the table plus its per-subquery aliases
            // (`bsl`, `bsl2`, `bsl3`), so `b`/`b2` don't answer for them.
            let is_link = target == link_table
                || (target.starts_with(link_alias)
                    && target[link_alias.len()..]
                        .chars()
                        .all(|c| c.is_ascii_digit()));
            if !is_link {
                continue;
            }
            match verb {
                "SCAN" => scans += 1,
                _ => assert!(
                    line.contains("USING") && line.contains("INDEX"),
                    "{arm} plan reads the link table without an index:\n{plan}"
                ),
            }
        }
        assert!(
            scans <= 1,
            "{arm} plan reads the link table {scans} times; only the \
             `effective` membership pass may:\n{plan}"
        );
    }
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
