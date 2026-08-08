//! Tests for `sync_delta`: per-device entitlement change tracking — new,
//! changed, and removed books since the last snapshot, ordering (removals
//! after adds/changes), idempotent recording, snapshot clearing, and
//! cascading cleanup when a device is deleted.

use omnibus_shared::{CreateShelfRequest, ShelfKind};

use super::*;
use crate::init_db;
use crate::shelves::{create_shelf, update_shelf};
use crate::test_support::seed_synced_ebook;

async fn make_user(pool: &SqlitePool, username: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin) VALUES (?, 'x', 0) RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn make_device(pool: &SqlitePool, user_id: i64, name: &str) -> i64 {
    crate::kobo_devices::create_device(pool, user_id, name)
        .await
        .unwrap()
        .id
}

/// A hand-picked shelf holding `uuids`, flagged for Kobo sync. Returns its id.
async fn synced_shelf(pool: &SqlitePool, owner: i64, name: &str, uuids: &[String]) -> i64 {
    let shelf = create_shelf(
        pool,
        owner,
        &CreateShelfRequest {
            kind: ShelfKind::Manual,
            name: name.into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: Vec::new(),
            book_uuids: uuids.to_vec(),
        },
    )
    .await
    .unwrap();
    update_shelf(
        pool,
        shelf.id,
        &omnibus_shared::UpdateShelfRequest {
            sync_to_kobo: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    shelf.id
}

/// Drive one full sync: compute the delta, then commit it.
async fn sync_once(pool: &SqlitePool, user: i64, device: i64) -> SyncDelta {
    let delta = sync_delta(pool, user, device).await.unwrap();
    record_synced(pool, device, &delta.changes).await.unwrap();
    delta
}

#[tokio::test]
async fn sync_delta_emits_new_entitlements_on_a_devices_first_sync() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;

    let delta = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(delta.len(), 1);
    assert!(matches!(&delta.changes[0], SyncChange::New(b) if b.uuid == uuid));
}

#[tokio::test]
async fn sync_delta_is_empty_on_a_second_sync_with_no_changes() {
    // The whole point of the snapshot: a device that already holds everything
    // gets nothing, instead of re-downloading the library on every poll.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;

    assert_eq!(sync_once(&pool, user, device).await.len(), 1);

    assert!(sync_delta(&pool, user, device).await.unwrap().is_empty());
}

#[tokio::test]
async fn sync_delta_emits_a_change_when_last_modified_advances() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;

    sqlx::query("UPDATE books SET last_modified = 9999999999 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let delta = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(delta.len(), 1);
    assert!(matches!(&delta.changes[0], SyncChange::Changed(b) if b.uuid == uuid));
}

#[tokio::test]
async fn sync_delta_emits_a_removal_when_a_book_leaves_the_opted_in_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let shelf = synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;

    // Opt the shelf back out; the book is still indexed, just no longer synced.
    update_shelf(
        &pool,
        shelf,
        &omnibus_shared::UpdateShelfRequest {
            sync_to_kobo: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let delta = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(delta.len(), 1);
    assert!(matches!(
        &delta.changes[0],
        SyncChange::Removed { book_uuid } if *book_uuid == uuid
    ));
}

#[tokio::test]
async fn sync_delta_orders_removals_after_adds_and_changes() {
    // A device applies the batch in order; archiving before the adds land would
    // briefly empty a book the same sync re-adds.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let going = seed_synced_ebook(&pool, "going.epub", "Going", "A").await;
    let shelf = synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&going)).await;
    sync_once(&pool, user, device).await;

    let arriving = seed_synced_ebook(&pool, "new.epub", "Arriving", "B").await;
    crate::shelves::remove_book(&pool, shelf, &going)
        .await
        .unwrap();
    crate::shelves::add_books(&pool, shelf, std::slice::from_ref(&arriving), user)
        .await
        .unwrap();

    let delta = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(delta.len(), 2);
    assert!(matches!(&delta.changes[0], SyncChange::New(b) if b.uuid == arriving));
    assert!(matches!(
        &delta.changes[1],
        SyncChange::Removed { book_uuid } if *book_uuid == going
    ));
}

#[tokio::test]
async fn sync_delta_does_not_advance_the_snapshot_on_its_own() {
    // `sync_delta` is read-only so a device that drops the connection mid-body
    // sees the same delta again rather than silently losing the book.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;

    let first = sync_delta(&pool, user, device).await.unwrap();
    let second = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1, "delta must survive an uncommitted sync");
}

#[tokio::test]
async fn sync_delta_is_scoped_per_device() {
    // Two devices on one account track independently — syncing one must not
    // mark the book as delivered to the other.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let clara = make_device(&pool, user, "Clara").await;
    let libra = make_device(&pool, user, "Libra").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;

    sync_once(&pool, user, clara).await;

    let libra_delta = sync_delta(&pool, user, libra).await.unwrap();
    assert_eq!(libra_delta.len(), 1);
    assert!(matches!(&libra_delta.changes[0], SyncChange::New(_)));
}

#[tokio::test]
async fn record_synced_is_idempotent_for_a_repeated_change() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;

    let delta = sync_delta(&pool, user, device).await.unwrap();
    record_synced(&pool, device, &delta.changes).await.unwrap();
    record_synced(&pool, device, &delta.changes).await.unwrap();

    let held: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kobo_books_sync WHERE device_id = ?")
        .bind(device)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(held, 1);
}

#[tokio::test]
async fn clear_snapshot_makes_the_next_sync_resend_everything() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;
    assert!(sync_delta(&pool, user, device).await.unwrap().is_empty());

    clear_snapshot(&pool, device).await.unwrap();

    assert_eq!(sync_delta(&pool, user, device).await.unwrap().len(), 1);
}

#[tokio::test]
async fn deleting_a_device_cascades_its_snapshot() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;

    crate::kobo_devices::revoke_device(&pool, user, device)
        .await
        .unwrap();

    let orphans: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kobo_books_sync WHERE device_id = ?")
            .bind(device)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(orphans, 0);
}

#[tokio::test]
async fn sync_delta_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    pool.close().await;

    assert!(matches!(
        sync_delta(&pool, user, device).await,
        Err(KoboError::Sqlx(_))
    ));
}

#[tokio::test]
async fn sync_delta_emits_a_state_change_when_read_status_moves_elsewhere() {
    // A book marked Finished on the web must be re-announced to a device that
    // already holds it — its metadata never changed, so the `Changed` arm
    // alone would stay silent forever.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;
    assert!(sync_delta(&pool, user, device).await.unwrap().is_empty());

    // `synced_at` has 1-second granularity, so age the snapshot rather than
    // racing it — the assertion is about the comparison, not the clock.
    sqlx::query("UPDATE kobo_books_sync SET synced_at = 1 WHERE device_id = ?")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    crate::read_status::set_read_status(
        &pool,
        user,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: omnibus_shared::ReadStatus::Finished,
        },
    )
    .await
    .unwrap();

    let delta = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(delta.len(), 1);
    assert!(matches!(&delta.changes[0], SyncChange::StateChanged(b) if b.uuid == uuid));
}

#[tokio::test]
async fn recording_a_state_change_preserves_last_modified_seen() {
    // The state watermark and the metadata watermark are independent: bumping
    // the former must not mark unsent metadata as already delivered.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;
    let seen_before: i64 = sqlx::query_scalar(
        "SELECT last_modified_seen FROM kobo_books_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();

    let row = crate::kobo::book_for_sync(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();
    record_synced(&pool, device, &[SyncChange::StateChanged(row)])
        .await
        .unwrap();

    let seen_after: i64 = sqlx::query_scalar(
        "SELECT last_modified_seen FROM kobo_books_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seen_after, seen_before, "metadata watermark must not move");
}

/// A synthetic row for the batching tests below. `kobo_books_sync` has no FK
/// to `books`, so these never need a real indexed book behind them.
fn synthetic_book(n: usize, last_modified_epoch: i64) -> KoboBookRow {
    KoboBookRow {
        id: n as i64,
        uuid: format!("synthetic-{n:05}"),
        title: format!("Book {n}"),
        author: "Author".into(),
        last_modified_epoch,
        download_size_bytes: 0,
        has_epub: true,
    }
}

#[tokio::test]
async fn record_synced_applies_every_group_across_multiple_chunks() {
    // Large enough in every bucket (upsert, state-changed, removed) to force
    // more than one chunk each, so a bug in the per-chunk boundary — a
    // dropped row, a double-bound param, an off-by-one in the WHERE IN list —
    // would show up as a wrong count or a wrong value here.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;

    // Seed 1950 pre-existing rows: 50 will be updated (`Changed`), 950 will
    // have their reading-state watermark bumped, 950 will be removed.
    let seed: Vec<SyncChange> = (0..1950)
        .map(|n| SyncChange::New(synthetic_book(n, 1)))
        .collect();
    record_synced(&pool, device, &seed).await.unwrap();

    let mut batch = Vec::new();
    batch.extend((0..350).map(|n| SyncChange::New(synthetic_book(2000 + n, 1))));
    batch.extend((0..50).map(|n| SyncChange::Changed(synthetic_book(n, 42))));
    batch.extend((50..1000).map(|n| SyncChange::StateChanged(synthetic_book(n, 1))));
    batch.extend((1000..1950).map(|n| SyncChange::Removed {
        book_uuid: format!("synthetic-{n:05}"),
    }));
    assert_eq!(
        batch.len(),
        2300,
        "sanity: every bucket crosses its chunk size"
    );

    record_synced(&pool, device, &batch).await.unwrap();

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kobo_books_sync WHERE device_id = ?")
        .bind(device)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 1950 + 350 - 950, "new rows land, removed rows drop");

    let changed_seen: i64 = sqlx::query_scalar(
        "SELECT last_modified_seen FROM kobo_books_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind("synthetic-00000")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(changed_seen, 42, "a Changed row's watermark must advance");

    let state_changed_seen: i64 = sqlx::query_scalar(
        "SELECT last_modified_seen FROM kobo_books_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind("synthetic-00500")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        state_changed_seen, 1,
        "a StateChanged row's metadata watermark must not move"
    );

    let last_new: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM kobo_books_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind("synthetic-02349")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(last_new, 1, "the last chunk of new rows must land");

    let removed_gone: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM kobo_books_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind("synthetic-01949")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(removed_gone, 0, "the last chunk of removals must apply");
}

#[tokio::test]
async fn sync_delta_prefers_a_metadata_change_over_a_state_change() {
    // When both moved, the device needs the full `Changed` pair — a bare
    // reading-state item would leave its metadata stale.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = make_user(&pool, "reader").await;
    let device = make_device(&pool, user, "Clara").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    synced_shelf(&pool, user, "Kobo", std::slice::from_ref(&uuid)).await;
    sync_once(&pool, user, device).await;

    sqlx::query("UPDATE kobo_books_sync SET synced_at = 1 WHERE device_id = ?")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET last_modified = 9999999999 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();
    crate::read_status::set_read_status(
        &pool,
        user,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: omnibus_shared::ReadStatus::Reading,
        },
    )
    .await
    .unwrap();

    let delta = sync_delta(&pool, user, device).await.unwrap();

    assert_eq!(delta.len(), 1);
    assert!(matches!(&delta.changes[0], SyncChange::Changed(_)));
}
