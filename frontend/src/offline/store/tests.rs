//! Unit tests for the offline SQLite store worker.

use super::*;

fn open_temp() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("offline.db")).expect("open store");
    (dir, store)
}

fn download_row(uuid: &str, format: &str) -> DownloadRow {
    DownloadRow {
        book_uuid: uuid.to_string(),
        format: format.to_string(),
        title: "T".to_string(),
        file_id: None,
        status: "downloading".to_string(),
        total_bytes: Some(1000),
        downloaded_bytes: 0,
        files: "[]".to_string(),
        error: None,
        updated_at: now_secs(),
    }
}

#[tokio::test]
async fn open_creates_schema_and_kv_round_trips() {
    let (_dir, store) = open_temp();
    assert!(store.kv_get("missing").await.is_none());
    store.kv_put("ebook:abc", "{\"title\":\"T\"}".to_string());
    let row = store.kv_get("ebook:abc").await.expect("row");
    assert_eq!(row.payload, "{\"title\":\"T\"}");
    assert!(row.fetched_at > 0);
}

#[tokio::test]
async fn kv_put_overwrites_existing_payload() {
    let (_dir, store) = open_temp();
    store.kv_put("k", "old".to_string());
    store.kv_put("k", "new".to_string());
    assert_eq!(store.kv_get("k").await.expect("row").payload, "new");
}

#[tokio::test]
async fn kv_delete_prefix_removes_only_matching_keys() {
    let (_dir, store) = open_temp();
    store.kv_put("highlights:a", "1".to_string());
    store.kv_put("highlights:b", "2".to_string());
    store.kv_put("shelves", "3".to_string());
    store.kv_delete_prefix("highlights:");
    assert!(store.kv_get("highlights:a").await.is_none());
    assert!(store.kv_get("highlights:b").await.is_none());
    assert!(store.kv_get("shelves").await.is_some());
}

#[tokio::test]
async fn kv_rename_moves_payload_and_replaces_target() {
    let (_dir, store) = open_temp();
    store.kv_put("shelf:-1", "temp".to_string());
    store.kv_put("shelf:9", "stale".to_string());
    store.kv_rename("shelf:-1", "shelf:9");
    assert!(store.kv_get("shelf:-1").await.is_none());
    assert_eq!(store.kv_get("shelf:9").await.expect("row").payload, "temp");
}

#[tokio::test]
async fn ops_append_preserves_enqueue_order() {
    let (_dir, store) = open_temp();
    store.ops_append("A", "1".to_string(), None);
    store.ops_append("B", "2".to_string(), None);
    store.ops_append("C", "3".to_string(), None);
    let kinds: Vec<String> = store.ops_list().await.into_iter().map(|o| o.kind).collect();
    assert_eq!(kinds, vec!["A", "B", "C"]);
}

#[tokio::test]
async fn ops_append_coalesces_same_key_and_keeps_latest_payload() {
    let (_dir, store) = open_temp();
    store.ops_append(
        "SaveProgress",
        "p1".to_string(),
        Some("prog:u:epub".to_string()),
    );
    store.ops_append("Other", "x".to_string(), None);
    store.ops_append(
        "SaveProgress",
        "p2".to_string(),
        Some("prog:u:epub".to_string()),
    );
    let ops = store.ops_list().await;
    assert_eq!(ops.len(), 2);
    // The coalesced op re-enqueues at the tail with the latest payload.
    assert_eq!(ops[0].kind, "Other");
    assert_eq!(ops[1].payload, "p2");
}

#[tokio::test]
async fn ops_delete_many_and_count_track_the_queue() {
    let (_dir, store) = open_temp();
    store.ops_append("A", "1".to_string(), None);
    store.ops_append("B", "2".to_string(), None);
    assert_eq!(store.ops_count().await, 2);
    let ids: Vec<i64> = store.ops_list().await.into_iter().map(|o| o.id).collect();
    store.ops_delete_many(ids).await;
    assert_eq!(store.ops_count().await, 0);
}

#[tokio::test]
async fn ops_rewrite_updates_only_matched_payloads() {
    let (_dir, store) = open_temp();
    store.ops_append("Edit", "{\"id\":-1}".to_string(), None);
    store.ops_append("Edit", "{\"id\":4}".to_string(), None);
    store
        .ops_rewrite(|_kind, payload| payload.contains("-1").then(|| payload.replace("-1", "7")))
        .await;
    let payloads: Vec<String> = store
        .ops_list()
        .await
        .into_iter()
        .map(|o| o.payload)
        .collect();
    assert_eq!(payloads, vec!["{\"id\":7}", "{\"id\":4}"]);
}

#[tokio::test]
async fn next_temp_id_decrements_and_survives_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("offline.db");
    {
        let store = Store::open(path.clone()).expect("open");
        assert_eq!(store.next_temp_id().await, Some(-1));
        assert_eq!(store.next_temp_id().await, Some(-2));
        // Force the pending meta write to land before reopening.
        let _ = store.meta_get("temp_id_counter").await;
    }
    let store = Store::open(path).expect("reopen");
    assert_eq!(store.next_temp_id().await, Some(-3));
}

#[tokio::test]
async fn open_returns_sqlite_error_when_path_is_a_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = Store::open(dir.path().to_path_buf());
    assert!(matches!(result, Err(StoreError::Sqlite(_))));
}

#[test]
fn store_error_worker_spawn_variant_carries_the_source_message() {
    let io_err = std::io::Error::other("boom");
    let err = StoreError::WorkerSpawn(io_err);
    assert_eq!(
        err.to_string(),
        "could not spawn the offline store worker thread: boom"
    );
}

#[tokio::test]
async fn meta_round_trips_values() {
    let (_dir, store) = open_temp();
    assert!(store.meta_get("last_sync_at").await.is_none());
    store.meta_put("last_sync_at", "123".to_string());
    assert_eq!(store.meta_get("last_sync_at").await.as_deref(), Some("123"));
}

#[tokio::test]
async fn downloads_upsert_is_keyed_per_uuid_and_format() {
    let (_dir, store) = open_temp();
    store.downloads_upsert(download_row("u1", "epub")).await;
    store.downloads_upsert(download_row("u1", "audio")).await;
    let mut updated = download_row("u1", "epub");
    updated.status = "complete".to_string();
    updated.downloaded_bytes = 1000;
    store.downloads_upsert(updated.clone()).await;

    let all = store.downloads_all().await;
    assert_eq!(all.len(), 2);
    let epub = all.iter().find(|r| r.format == "epub").expect("epub row");
    assert_eq!(epub.status, "complete");
    let audio = all.iter().find(|r| r.format == "audio").expect("audio row");
    assert_eq!(audio.status, "downloading");
}

#[tokio::test]
async fn kv_delete_removes_only_the_named_key() {
    let (_dir, store) = open_temp();
    store.kv_put("a", "1".to_string());
    store.kv_put("b", "2".to_string());
    store.kv_delete("a");
    assert!(store.kv_get("a").await.is_none());
    assert_eq!(store.kv_get("b").await.expect("row").payload, "2");
}

#[tokio::test]
async fn kv_prefix_returns_only_matching_key_payload_pairs() {
    let (_dir, store) = open_temp();
    store.kv_put("progress:u1:epub", "1".to_string());
    store.kv_put("progress:u2:epub", "2".to_string());
    store.kv_put("shelves", "3".to_string());
    let mut rows = store.kv_prefix("progress:").await;
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("progress:u1:epub".to_string(), "1".to_string()),
            ("progress:u2:epub".to_string(), "2".to_string()),
        ]
    );
}

#[tokio::test]
async fn kv_prefix_blocking_returns_only_matching_key_payload_pairs() {
    let (_dir, store) = open_temp();
    store.kv_put("rating:u1", "4.5".to_string());
    store.kv_put("rating:u2", "3.0".to_string());
    store.kv_put("shelves", "x".to_string());
    // The put calls above go through the async worker channel; force them to
    // land before the blocking read races the same queue.
    let _ = store.kv_get("shelves").await;
    let mut rows = store.kv_prefix_blocking("rating:");
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("rating:u1".to_string(), "4.5".to_string()),
            ("rating:u2".to_string(), "3.0".to_string()),
        ]
    );
}

#[test]
fn run_blocking_returns_the_closures_value_from_the_worker_thread() {
    let (_dir, store) = open_temp();
    let result = store.run_blocking(|_conn| 42i64);
    assert_eq!(result, Some(42));
}

#[tokio::test]
async fn ops_bump_attempts_increments_only_the_named_op() {
    let (_dir, store) = open_temp();
    store.ops_append("A", "1".to_string(), None);
    store.ops_append("B", "2".to_string(), None);
    let ops = store.ops_list().await;
    let a_id = ops.iter().find(|o| o.kind == "A").expect("op A").id;
    store.ops_bump_attempts(a_id);
    // Force the detached write to land before reading it back.
    let _ = store.ops_count().await;
    let ops = store.ops_list().await;
    let a = ops.iter().find(|o| o.id == a_id).expect("op A");
    let b = ops.iter().find(|o| o.kind == "B").expect("op B");
    assert_eq!(a.attempts, 1);
    assert_eq!(b.attempts, 0);
}

#[tokio::test]
async fn downloads_delete_removes_only_the_named_format() {
    let (_dir, store) = open_temp();
    store.downloads_upsert(download_row("u1", "epub")).await;
    store.downloads_upsert(download_row("u1", "audio")).await;
    store.downloads_delete("u1", "epub").await;
    let all = store.downloads_all().await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].format, "audio");
}
