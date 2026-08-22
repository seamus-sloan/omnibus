//! The WARN-only checks in `upsert_progress_tx`: a rejected write and
//! a large backward audio jump each emit exactly one event, and nothing else
//! does. Uses a thread-scoped capturing subscriber.

use omnibus_shared::ProgressUpdate;

use crate::init_db;

use super::super::*;
use super::{audio_update, seed, seed_audio_files, seed_user};

// --- #1861: WARN logging on rejected writes and backward audio jumps ---

/// Captures every WARN-or-louder `tracing` event's message plus fields as
/// one formatted line, while installed as the default subscriber —
/// `set_default` scopes it to the current thread, so a single-threaded
/// runtime keeps a test's capture isolated from any other test running in
/// the process. Every test using this pattern pins
/// `#[tokio::test(flavor = "current_thread")]` explicitly rather than
/// relying on the tokio-macros default (`current_thread` for `#[tokio::test]`
/// regardless of the `rt-multi-thread` feature — that feature only makes
/// `flavor = "multi_thread"` available as an opt-in) so the reliance stays
/// true even if that default ever changed. Mirrors the `QueryCounter`
/// pattern in `db/src/epub_rewrite/tests.rs`; every `Subscriber` method
/// besides `event` is a no-op since this only needs to tally/record events,
/// never spans.
struct WarnCapture(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

struct FieldLine(String);

impl tracing::field::Visit for FieldLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={:?}", field.name(), value);
    }
}

impl tracing::Subscriber for WarnCapture {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.level() <= &tracing::Level::WARN
    }
    fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().level() > &tracing::Level::WARN {
            return;
        }
        let mut line = FieldLine(String::new());
        event.record(&mut line);
        self.0.lock().unwrap().push(line.0);
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[tokio::test(flavor = "current_thread")]
async fn upsert_progress_tx_logs_warn_when_write_rejected_by_timestamp_guard() {
    // AC1 (#1861): a rejected write must emit one WARN naming the book and
    // both timestamps, so a revert like the 2026-08-11 chapter-19-to-13 one
    // leaves a trace even though the response silently carries the winner.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(newer)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(2000),
        },
    )
    .await
    .unwrap();

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(WarnCapture(events.clone()));
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(stale-offline-replay)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1000),
        },
    )
    .await
    .unwrap();
    drop(guard);

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1, "exactly one WARN, got {captured:?}");
    let line = &captured[0];
    assert!(
        line.contains(&format!("book_uuid={uuid:?}")),
        "missing book uuid: {line}"
    );
    assert!(
        line.contains("stored_client_updated_at=2000"),
        "missing stored stamp: {line}"
    );
    assert!(
        line.contains("offered_client_updated_at=1000"),
        "missing offered stamp: {line}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn upsert_progress_tx_logs_warn_when_accepted_audio_write_jumps_backward_past_threshold() {
    // AC2 (#1861): an accepted write that moves the audio position backward
    // by more than the ~10-minute threshold must emit one WARN with both
    // positions.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 1).await;
    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 20_000.0, Some(files[0]), 100),
    )
    .await
    .unwrap();

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(WarnCapture(events.clone()));
    let saved = upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 100.0, Some(files[0]), 200),
    )
    .await
    .unwrap();
    drop(guard);

    assert_eq!(saved.audio_position_seconds, Some(100.0), "write must land");
    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1, "exactly one WARN, got {captured:?}");
    let line = &captured[0];
    // Unlike the timestamp-guard WARN above, this call site formats
    // `book_uuid` with `%` (Display), so the captured field carries no
    // Debug-quoting — see `warn_if_audio_jumped_backward` in progress.rs.
    assert!(
        line.contains(&format!("book_uuid={uuid}")),
        "missing book uuid: {line}"
    );
    assert!(
        line.contains("old_position_seconds=20000.0"),
        "missing old position: {line}"
    );
    assert!(
        line.contains("new_position_seconds=100.0"),
        "missing new position: {line}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn upsert_progress_tx_logs_nothing_for_a_normal_forward_write() {
    // AC3 (#1861): a plain forward write — newer timestamp, position moving
    // ahead — must not emit either WARN.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 1).await;
    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 100.0, Some(files[0]), 100),
    )
    .await
    .unwrap();

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(WarnCapture(events.clone()));
    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 400.0, Some(files[0]), 200),
    )
    .await
    .unwrap();
    drop(guard);

    assert!(
        events.lock().unwrap().is_empty(),
        "a forward write must not log a new WARN"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn upsert_progress_tx_logs_nothing_for_a_small_backward_audio_seek() {
    // A normal skip-back / chapter re-listen (well under the ~10-minute
    // threshold) is ordinary playback, not a revert — must stay silent.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 1).await;
    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 1_000.0, Some(files[0]), 100),
    )
    .await
    .unwrap();

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(WarnCapture(events.clone()));
    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 970.0, Some(files[0]), 200),
    )
    .await
    .unwrap();
    drop(guard);

    assert!(
        events.lock().unwrap().is_empty(),
        "a 30s skip-back must not cross the ~10-minute threshold"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn upsert_progress_tx_logs_nothing_for_the_first_write_to_a_book() {
    // No prior row means no "old" position to compare against — the very
    // first write for a `(user, book, format)` must never look like a jump.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 1).await;

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(WarnCapture(events.clone()));
    upsert_progress(&pool, user, &audio_update(&uuid, 5.0, Some(files[0]), 100))
        .await
        .unwrap();
    drop(guard);

    assert!(
        events.lock().unwrap().is_empty(),
        "the first write for a book must not log a WARN"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn upsert_progress_tx_logs_nothing_when_a_write_is_rejected_by_the_audio_file_guard() {
    // The multi-file-audio-guard rejection (#1888) is a different rejection
    // reason than the timestamp guard, and out of this issue's scope — it
    // must not be misreported as a timestamp-guard rejection.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 2).await;
    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 23_718.0, Some(files[1]), 100),
    )
    .await
    .unwrap();

    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(WarnCapture(events.clone()));
    let survived = upsert_progress(&pool, user, &audio_update(&uuid, 60.0, None, 200))
        .await
        .unwrap();
    drop(guard);

    assert_eq!(
        survived.audio_position_seconds,
        Some(23_718.0),
        "the audio-file guard must still reject the write"
    );
    assert!(
        events.lock().unwrap().is_empty(),
        "the audio-file-guard rejection is out of #1861's scope"
    );
}
