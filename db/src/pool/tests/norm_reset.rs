//! The derived-key resets on upgrade: 0070 recomputes stale ampersand
//! norms and 0071 merges the stale embedded-index series, each driven the
//! way a real install takes them.

use super::super::*;

/// Migration 0070 plus the boot backfill re-derive the keys the `&`
/// expansion invalidated. Simulates an upgrade over an existing database:
/// rows written by the old folder (which dropped `&`) and a
/// `_sqlx_migrations` table that has not yet seen 0070, then a second
/// `init_db` over the same file.
///
/// Covers all four shapes the two arms have to get right: a `&` title with a
/// link, a `&` title *without* one (the blocklisted-first-creator gap, where
/// the stored author key is the only copy and must survive), a `&` in the
/// position-0 author's name, and a row with no `&` at all.
#[tokio::test]
async fn migration_0070_recomputes_stale_ampersand_norms_on_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = init_db(&url).await.expect("initial init_db should succeed");

    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    // Every `*_norm` seeded here is what the pre-change folder wrote: the
    // ampersand collapsed to a space like any other punctuation. The two
    // `sentinel` keys are deliberately wrong, so any recompute shows up.
    let ampersand_title = "A Tale of Mirth & Magic";
    seed_pre_ampersand_book(
        &pool,
        "u1",
        ampersand_title,
        "a tale of mirth magic",
        "ada quill",
        Some("Ada Quill"),
    )
    .await;
    seed_pre_ampersand_book(
        &pool,
        "u2",
        "Dracula",
        "sentinel",
        "sentinel",
        Some("Ada Quill"),
    )
    .await;
    seed_pre_ampersand_book(
        &pool,
        "u3",
        ampersand_title,
        "a tale of mirth magic",
        "blocklisted quill",
        None,
    )
    .await;
    seed_pre_ampersand_book(
        &pool,
        "u4",
        "Duet",
        "duet",
        "vale quill",
        Some("Vale & Quill"),
    )
    .await;
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, title_norm, author_norm)
         VALUES ('u1', '{\"title\":\"A Tale of Mirth & Magic\"}', 'a tale of mirth magic', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Hand the database back its pre-0070 migration state and re-open it.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 70")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);
    let pool = init_db(&url).await.expect("upgrade init_db should succeed");

    assert_eq!(
        norms(&pool, "u1").await,
        ("a tale of mirth and magic".into(), Some("ada quill".into())),
        "a `&` title heals; its derivable author key is unchanged"
    );
    assert_eq!(
        norms(&pool, "u2").await,
        ("sentinel".into(), Some("sentinel".into())),
        "a row with no `&` on either side is not reset at all"
    );
    assert_eq!(
        norms(&pool, "u3").await,
        (
            "a tale of mirth and magic".into(),
            Some("blocklisted quill".into())
        ),
        "a `&` title must not cost a book its non-derivable author key"
    );
    assert_eq!(
        norms(&pool, "u4").await,
        ("duet".into(), Some("vale and quill".into())),
        "a `&` in the position-0 author's name heals the author key"
    );
    // The override keys need no migration — their boot pass recomputes every
    // row from the `overrides` JSON and rewrites the ones that disagree.
    let override_norm: String =
        sqlx::query_scalar("SELECT title_norm FROM metadata_overrides WHERE book_uuid = 'u1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(override_norm, "a tale of mirth and magic");
}

/// #1912 / AC3: pre-fix rows whose series metadata carried the index inside
/// the name ("Crowns of Nyaxia #1"/"#2"/"#3") fragmented into three
/// one-book series, `series_index` never filled. Migration 0071 nulls the
/// stale `series_sort` those rows already hold and the boot backfills
/// (`series_normalize::backfill_embedded_series_index`, then
/// `sort_keys::backfill_series_sort`) merge the fragmented `series` rows
/// into one and fill each book's index. Simulates an upgrade the same way
/// `migration_0070_recomputes_stale_ampersand_norms_on_upgrade` does: rows
/// seeded as the pre-fix folder wrote them, then a second `init_db` over the
/// same file with 0071 rewound.
#[tokio::test]
async fn migration_0071_merges_stale_embedded_index_series_on_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = init_db(&url).await.expect("initial init_db should succeed");

    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    for (uuid, n) in [("u1", 1), ("u2", 2), ("u3", 3)] {
        let series_name = format!("Crowns of Nyaxia #{n}");
        let series_id: i64 =
            sqlx::query_scalar("INSERT INTO series (name) VALUES (?1) RETURNING id")
                .bind(&series_name)
                .fetch_one(&pool)
                .await
                .unwrap();
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books (uuid, library_id, path, title, series_sort)
             VALUES (?1, ?2, '', ?3, ?4) RETURNING id",
        )
        .bind(uuid)
        .bind(lib_id)
        .bind(format!("Book {n}"))
        .bind(&series_name)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?1, ?2)")
            .bind(book_id)
            .bind(series_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Hand the database back its pre-0071 migration state and re-open it.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 71")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);
    let pool = init_db(&url).await.expect("upgrade init_db should succeed");

    let series_rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        series_rows.len(),
        1,
        "AC3: the three fragmented rows converge on one series: {series_rows:?}"
    );
    assert_eq!(series_rows[0].1, "Crowns of Nyaxia");

    let mut rows: Vec<(String, f64, Option<String>)> =
        sqlx::query_as("SELECT title, series_index, series_sort FROM books ORDER BY series_index")
            .fetch_all(&pool)
            .await
            .unwrap();
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    assert_eq!(
        rows,
        vec![
            (
                "Book 1".to_string(),
                1.0,
                Some("Crowns of Nyaxia".to_string())
            ),
            (
                "Book 2".to_string(),
                2.0,
                Some("Crowns of Nyaxia".to_string())
            ),
            (
                "Book 3".to_string(),
                3.0,
                Some("Crowns of Nyaxia".to_string())
            ),
        ],
        "each book keeps its parsed index and gets the cleaned series_sort"
    );
}

/// Seed a book carrying hand-written `_norm` keys, standing in for a row the
/// pre-`&`-expansion folder wrote. `link_author` is the position-0 author link
/// the boot backfill re-derives `author_norm` from; `None` reproduces the
/// blocklisted-first-creator gap, where the link is absent and the stored key
/// is the only copy of it.
async fn seed_pre_ampersand_book(
    pool: &SqlitePool,
    uuid: &str,
    title: &str,
    title_norm: &str,
    author_norm: &str,
    link_author: Option<&str>,
) {
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, title_norm, author_norm)
         VALUES (?1, (SELECT id FROM scan_roots WHERE path = '/lib'), '', ?2, ?3, ?4)
         RETURNING id",
    )
    .bind(uuid)
    .bind(title)
    .bind(title_norm)
    .bind(author_norm)
    .fetch_one(pool)
    .await
    .unwrap();
    let Some(name) = link_author else { return };
    sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?1)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books_authors_link (book, author, position)
         VALUES (?1, (SELECT id FROM authors WHERE name = ?2), 0)",
    )
    .bind(book_id)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

/// The stored `(title_norm, author_norm)` pair for one book.
async fn norms(pool: &SqlitePool, uuid: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE uuid = ?1")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}
