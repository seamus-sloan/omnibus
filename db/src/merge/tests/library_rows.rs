//! Unit tests for the library-wide rows a merge carries onto the target —
//! shelf memberships, copies, wishes, the cross-format link, the content
//! index — and the schema guard that holds every `book_uuid` table to the
//! retarget-or-exempt contract.

use super::super::*;
use super::seed_user;
use crate::pool::init_db;
use crate::test_support::{
    seed_synced_audiobook as seed_audiobook, seed_synced_ebook as seed_ebook,
};

async fn seed_shelf(pool: &sqlx::SqlitePool, owner: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO shelves (owner_user_id, kind, name) VALUES (?, 'manual', ?) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn shelve(pool: &sqlx::SqlitePool, shelf: i64, uuid: &str, position: i64) {
    sqlx::query("INSERT INTO shelf_books (shelf_id, book_uuid, position) VALUES (?, ?, ?)")
        .bind(shelf)
        .bind(uuid)
        .bind(position)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn merge_moves_shelf_membership_and_keeps_the_lower_slot_on_a_shared_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let shared = seed_shelf(&pool, user, "Gothic").await;
    let source_only = seed_shelf(&pool, user, "Audio").await;
    shelve(&pool, shared, &target, 3).await;
    shelve(&pool, shared, &source, 1).await;
    shelve(&pool, source_only, &source, 0).await;

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT shelf_id, book_uuid, position FROM shelf_books ORDER BY shelf_id, position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (shared, target.clone(), 1),
            (source_only, target.clone(), 0)
        ],
        "the book stays on every shelf either edition was on, at the earlier slot"
    );
}

#[tokio::test]
async fn merge_moves_copies_and_wishlist_entries_and_keeps_the_earlier_wish() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash) VALUES ('reader', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    // The copy first: checking one in clears every wish for that book.
    crate::physical::add_physical_copy(&pool, &source, Some("9780000000001"), None, None)
        .await
        .unwrap();
    for (who, uuid, added_at, via) in [
        (user, &target, 2_000, "detail"),
        (user, &source, 1_000, "scan"),
        (other, &source, 3_000, "search"),
    ] {
        sqlx::query(
            "INSERT INTO wishlist_entries (user_id, book_uuid, added_at, source)
             VALUES (?, ?, ?, ?)",
        )
        .bind(who)
        .bind(uuid)
        .bind(added_at)
        .bind(via)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let copies: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT book_uuid, isbn FROM physical_copies")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        copies,
        vec![(target.clone(), Some("9780000000001".to_string()))]
    );

    let wishes: Vec<(i64, String, i64, String)> = sqlx::query_as(
        "SELECT user_id, book_uuid, added_at, source FROM wishlist_entries ORDER BY user_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        wishes,
        vec![
            (user, target.clone(), 1_000, "scan".to_string()),
            (other, target.clone(), 3_000, "search".to_string()),
        ],
        "one wish per reader survives, the earlier of the two"
    );
}

#[tokio::test]
async fn merge_keeps_the_targets_cross_format_link_on_a_collision() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash) VALUES ('reader', 'x') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    for (who, uuid, confirmed_at) in [
        (user, &target, 100),
        (user, &source, 200),
        (other, &source, 300),
    ] {
        sqlx::query(
            "INSERT INTO cross_format_links (user_id, book_uuid, mode, audio_snapshot, confirmed_at)
             VALUES (?, ?, 'sequence', '[]', ?)",
        )
        .bind(who)
        .bind(uuid)
        .bind(confirmed_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let links: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT user_id, book_uuid, confirmed_at FROM cross_format_links ORDER BY user_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        links,
        vec![(user, target.clone(), 100), (other, target.clone(), 300)],
        "the survivor's own confirmation stands; a reader who linked only the source keeps theirs"
    );
}

#[tokio::test]
async fn merge_carries_the_content_index_with_the_moved_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    for (uuid, spine, text) in [
        (&target, 0, "target chapter one"),
        (&source, 0, "source chapter one"),
        (&source, 1, "source chapter two"),
    ] {
        sqlx::query(
            "INSERT INTO book_content_chapters (book_uuid, spine_index, mtime_epoch, size_bytes, text)
             VALUES (?, ?, 1, 1, ?)",
        )
        .bind(uuid)
        .bind(spine)
        .bind(text)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let chapters: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT book_uuid, spine_index, text FROM book_content_chapters ORDER BY spine_index",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        chapters,
        vec![
            (target.clone(), 0, "target chapter one".to_string()),
            (target.clone(), 1, "source chapter two".to_string()),
        ]
    );
    // The external-content index followed through the triggers: a search
    // for the moved chapter resolves to the survivor.
    let hit: Option<String> = sqlx::query_scalar(
        "SELECT c.book_uuid FROM book_content_fts f
           JOIN book_content_chapters c ON c.id = f.rowid
          WHERE book_content_fts MATCH 'two'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(hit.as_deref(), Some(target.as_str()));
}

/// The guard on the soft-reference contract: every table keyed on
/// `book_uuid` is either retargeted or named exempt with a reason, so a new
/// one cannot join the stranded set silently.
#[tokio::test]
async fn merge_settles_every_book_uuid_table() {
    use super::super::transaction::{COLLISION_TABLES, MERGE_EXEMPT_TABLES, RETARGET_TABLES};

    let pool = init_db("sqlite::memory:").await.unwrap();
    let keyed: Vec<String> = sqlx::query_scalar(
        "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) p
          WHERE m.type = 'table' AND p.name = 'book_uuid'
          ORDER BY m.name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        !keyed.is_empty(),
        "the schema enumeration itself came back empty"
    );

    let retargeted: std::collections::HashSet<&str> = RETARGET_TABLES.into_iter().collect();
    let exempt: std::collections::HashSet<&str> =
        MERGE_EXEMPT_TABLES.iter().map(|(t, _)| *t).collect();
    for table in &keyed {
        assert!(
            retargeted.contains(table.as_str()) || exempt.contains(table.as_str()),
            "`{table}` carries a book_uuid column but the merge neither retargets it \
             nor lists it in MERGE_EXEMPT_TABLES with a reason"
        );
    }
    for table in retargeted.iter().chain(exempt.iter()) {
        assert!(
            keyed.iter().any(|k| k == table),
            "`{table}` is listed for the merge but no such book_uuid-keyed table exists"
        );
    }
    for collision in &COLLISION_TABLES {
        assert!(
            retargeted.contains(collision.table),
            "`{}` settles a collision for a table the merge does not retarget",
            collision.table
        );
    }
}
