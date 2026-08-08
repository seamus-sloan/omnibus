//! Tests for the in-memory error ring buffer.
//!
//! `record`/`snapshot` touch one process-wide static, so every test acquires
//! `TEST_LOCK` and clears the buffer first — otherwise parallel test threads
//! would race on shared state (mirrors `test_support::ENV_LOCK`).
use super::*;

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn entry(message: &str) -> CapturedError {
    CapturedError {
        timestamp: 1_700_000_000,
        target: "omnibus::test".to_string(),
        message: message.to_string(),
        book_uuid: None,
        file: None,
    }
}

#[test]
fn snapshot_returns_empty_when_buffer_untouched() {
    let _guard = lock_unpoison(&TEST_LOCK);
    clear();

    assert!(snapshot().is_empty());
}

#[test]
fn record_evicts_oldest_entry_once_buffer_is_at_capacity() {
    let _guard = lock_unpoison(&TEST_LOCK);
    clear();

    for i in 0..ERROR_RING_CAPACITY + 5 {
        record(entry(&format!("error {i}")));
    }

    let snap = snapshot();
    assert_eq!(snap.len(), ERROR_RING_CAPACITY, "buffer must stay capped");
    // The oldest 5 entries (0..5) should have been evicted first-in.
    assert!(
        snap.iter().all(|e| e.message != "error 0"),
        "oldest entry should have been evicted"
    );
    assert!(
        snap.iter()
            .any(|e| e.message == format!("error {}", ERROR_RING_CAPACITY + 4)),
        "newest entry should still be present"
    );
}

#[test]
fn snapshot_returns_entries_newest_first() {
    let _guard = lock_unpoison(&TEST_LOCK);
    clear();

    record(entry("first"));
    record(entry("second"));
    record(entry("third"));

    let snap = snapshot();
    let messages: Vec<&str> = snap.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, vec!["third", "second", "first"]);
}

#[test]
fn record_preserves_book_uuid_and_file_fields() {
    let _guard = lock_unpoison(&TEST_LOCK);
    clear();

    record(CapturedError {
        timestamp: 1,
        target: "omnibus::test".to_string(),
        message: "boom".to_string(),
        book_uuid: Some("abc-123".to_string()),
        file: Some("/lib/book.epub".to_string()),
    });

    let snap = snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].book_uuid.as_deref(), Some("abc-123"));
    assert_eq!(snap[0].file.as_deref(), Some("/lib/book.epub"));
}
