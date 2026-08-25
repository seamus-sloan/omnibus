//! The "Recently Interacted" axis: that each of the seven signals moves a
//! book, that a private journal draft does not, and that keyset paging over
//! the derived expression stays consistent with an unpaged read.

use omnibus_shared::{SortDir, SortKey, ViewFilters};
use sqlx::SqlitePool;

use crate::books::list_books_page;
use crate::pool::init_db;
use crate::test_support::{seed_minimal_books, seed_user};

/// `seed_minimal_books` numbers uuids `uuid-1`..`uuid-N`.
fn uuid(n: i64) -> String {
    format!("uuid-{n}")
}

/// Pin every book's clocks to a fixed, old epoch so a test's own signal is
/// unambiguously the most recent thing that happened to its book.
async fn flatten_clocks(pool: &SqlitePool) {
    sqlx::query("UPDATE books SET timestamp = 1000, last_modified = 1000")
        .execute(pool)
        .await
        .unwrap();
}

async fn interacted_at(pool: &SqlitePool, book_uuid: &str) -> Option<i64> {
    let sql = format!(
        "SELECT {} FROM books b WHERE b.uuid = ?",
        super::INTERACTED_AT_EPOCH
    );
    sqlx::query_scalar::<_, Option<i64>>(&sql)
        .bind(book_uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn seeded_pool(count: i64) -> SqlitePool {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, count).await;
    flatten_clocks(&pool).await;
    pool
}

#[tokio::test]
async fn interacted_at_falls_back_to_the_library_add_when_nothing_else_touched_the_book() {
    let pool = seeded_pool(1).await;
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(1000));
}

#[tokio::test]
async fn interacted_at_is_null_when_the_book_carries_no_signal_at_all() {
    let pool = seeded_pool(1).await;
    sqlx::query("UPDATE books SET timestamp = NULL, last_modified = NULL")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(interacted_at(&pool, &uuid(1)).await, None);
}

#[tokio::test]
async fn interacted_at_tracks_a_metadata_or_cover_edit_via_last_modified() {
    let pool = seeded_pool(1).await;
    // Both `save_overrides` and the cover upsert land here through
    // `touch_book_last_modified` — the two signals that need no join.
    sqlx::query("UPDATE books SET last_modified = 5000 WHERE uuid = ?")
        .bind(uuid(1))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(5000));
}

#[tokio::test]
async fn interacted_at_tracks_a_rating_by_any_user() {
    let pool = seeded_pool(1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, 8, 4000)",
    )
    .bind(user)
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(4000));
}

#[tokio::test]
async fn interacted_at_tracks_a_published_journal_entry() {
    let pool = seeded_pool(1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, status, created_at, updated_at)
         VALUES (?, ?, 'note', 'published', 3000, 4200)",
    )
    .bind(user)
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    // The later of created/updated wins — an edited entry is an interaction.
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(4200));
}

#[tokio::test]
async fn interacted_at_ignores_a_private_journal_draft() {
    let pool = seeded_pool(1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, status, created_at, updated_at)
         VALUES (?, ?, 'wip', 'draft', 9000, 9000)",
    )
    .bind(user)
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    // A draft must not reveal its existence by moving the book up a shared
    // sort, so the book stays at its library-add time.
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(1000));
}

#[tokio::test]
async fn interacted_at_tracks_a_read_status_change() {
    let pool = seeded_pool(1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at)
         VALUES (?, ?, 'reading', 6000)",
    )
    .bind(user)
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(6000));
}

#[tokio::test]
async fn interacted_at_tracks_a_physical_check_in() {
    let pool = seeded_pool(1).await;
    sqlx::query(
        "INSERT INTO physical_copies (book_uuid, isbn, checked_in_at) VALUES (?, '123', 7000)",
    )
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(7000));
}

#[tokio::test]
async fn interacted_at_takes_the_most_recent_signal_across_users_and_kinds() {
    let pool = seeded_pool(1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, 8, 4000)",
    )
    .bind(alice)
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at)
         VALUES (?, ?, 'finished', 8800)",
    )
    .bind(bob)
    .bind(uuid(1))
    .execute(&pool)
    .await
    .unwrap();
    // "By anyone" is the contract: a second user's later action wins.
    assert_eq!(interacted_at(&pool, &uuid(1)).await, Some(8800));
}

/// Walk every page of the axis and return the uuids in order.
async fn walk_pages(pool: &SqlitePool, dir: SortDir, limit: i64) -> Vec<String> {
    let filters = ViewFilters::default();
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let page = list_books_page(
            pool,
            &["/lib"],
            SortKey::RecentlyInteracted,
            dir,
            &filters,
            &[],
            cursor.as_ref(),
            limit,
        )
        .await
        .unwrap();
        seen.extend(
            page.books
                .iter()
                .filter_map(|b| b.unique_identifier.clone()),
        );
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn recently_interacted_paging_matches_an_unpaged_read_in_both_directions() {
    let pool = seeded_pool(9).await;
    let user = seed_user(&pool, "alice").await;
    // Give a subset distinct, interleaved interaction times; the rest keep
    // the flat library-add clock so the id tiebreak is exercised too.
    for (n, at) in [(2, 9000), (4, 3000), (5, 9000), (7, 1500), (9, 500)] {
        sqlx::query(
            "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
             VALUES (?, ?, 6, ?)",
        )
        .bind(user)
        .bind(uuid(n))
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }

    for dir in [SortDir::Desc, SortDir::Asc] {
        let unpaged = walk_pages(&pool, dir, 100).await;
        assert_eq!(unpaged.len(), 9, "{dir:?}: every book returned once");
        for limit in [1, 2, 4] {
            assert_eq!(
                walk_pages(&pool, dir, limit).await,
                unpaged,
                "{dir:?}: paging at limit {limit} must match the unpaged order"
            );
        }
    }
}

#[tokio::test]
async fn recently_interacted_orders_the_most_recently_touched_book_first() {
    let pool = seeded_pool(3).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at)
         VALUES (?, ?, 'reading', 9000)",
    )
    .bind(user)
    .bind(uuid(3))
    .execute(&pool)
    .await
    .unwrap();
    let order = walk_pages(&pool, SortDir::Desc, 100).await;
    assert_eq!(order.first().map(String::as_str), Some("uuid-3"));
}
