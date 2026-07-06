//! Per-task-kind handlers. `Worker::execute` is the single dispatch
//! match — each arm pulls the inputs out of a [`Task`] variant and calls
//! the owning module (`indexer::reindex`, `author_photos::resolve`,
//! `thumbs::ensure_thumbnails_sync`).

use std::sync::Arc;

use super::types::{Task, TaskId, TaskOutcome, Worker};

/// Map a handler's `Result` into a [`TaskOutcome`], stringifying the error.
/// The `Ok` payload is discarded (some handlers return a path/id), so `Ok(())`
/// and `Ok(value)` collapse to [`TaskOutcome::Ok`] identically.
fn outcome_of<T, E: std::fmt::Display>(result: Result<T, E>) -> TaskOutcome {
    match result {
        Ok(_) => TaskOutcome::Ok,
        Err(e) => TaskOutcome::Err(e.to_string()),
    }
}

impl Worker {
    pub(super) async fn execute(self: &Arc<Self>, task: Task, id: TaskId) -> TaskOutcome {
        match task {
            Task::Scan { library_path } => outcome_of(
                crate::indexer::reindex_with_progress(
                    &self.pool,
                    &library_path,
                    |processed, total| {
                        self.report_progress(id, processed, Some(total));
                    },
                )
                .await,
            ),
            Task::ScanAudiobooks { library_path } => {
                self.handle_scan_audiobooks(library_path, id).await
            }
            Task::HlsTranscode {
                book_id,
                book_file_id,
                library_path,
                profile,
            } => outcome_of(
                crate::hls::transcode_book(
                    &self.pool,
                    book_id,
                    book_file_id,
                    &library_path,
                    &profile,
                )
                .await,
            ),
            Task::ResolveAuthorPhoto { author_id } => {
                outcome_of(crate::author_photos::resolve(&self.pool, author_id).await)
            }
            Task::RefetchAuthorPhotos => outcome_of(
                crate::author_photos::refetch_all(&self.pool, |processed, total| {
                    self.report_progress(id, processed, total);
                })
                .await,
            ),
            Task::BackfillChapters { library_path } => outcome_of(
                crate::indexer::backfill_chapters(&self.pool, &library_path, |processed, total| {
                    self.report_progress(id, processed, Some(total));
                })
                .await,
            ),
            Task::RebuildFtsIndex => outcome_of(crate::sync::rebuild_all_fts(&self.pool).await),
            Task::ResolveSuggestions { book_uuid } => {
                outcome_of(crate::suggestions::resolve(&self.pool, &book_uuid).await)
            }
            Task::KepubConvert { book_id } => {
                outcome_of(crate::kepub::convert_book(&self.pool, book_id).await)
            }
            Task::SendToKindle {
                book_id,
                book_file_id,
                recipient_email,
            } => outcome_of(
                crate::kindle::send(&self.pool, book_id, book_file_id, &recipient_email).await,
            ),
            Task::GenerateThumbs {
                book_id,
                last_modified_epoch,
            } => {
                self.handle_generate_thumbs(book_id, last_modified_epoch)
                    .await
            }
            #[cfg(test)]
            Task::Test {
                tag: _,
                latency_ms,
                on_run,
                on_done,
                ..
            } => handle_test_task(latency_ms, on_run, on_done).await,
        }
    }

    /// Reindex an audiobook library, then (on success) post the follow-up
    /// chapter-backfill task. The same resource key means the backfill waits
    /// until this task fully finishes (including `run`'s terminal-state write),
    /// but the post itself is instant.
    async fn handle_scan_audiobooks(
        self: &Arc<Self>,
        library_path: String,
        id: TaskId,
    ) -> TaskOutcome {
        match crate::indexer::reindex_audiobooks_with_progress(
            &self.pool,
            &library_path,
            |processed, total| {
                self.report_progress(id, processed, Some(total));
            },
        )
        .await
        {
            Ok(()) => {
                self.post(Task::BackfillChapters { library_path });
                TaskOutcome::Ok
            }
            Err(e) => TaskOutcome::Err(e.to_string()),
        }
    }

    /// Fetch a book's cover and (re)generate its WebP thumbnails on the
    /// blocking pool, then evict the thumb cache if it's over the cap. A
    /// missing cover is an error; a `JoinError` distinguishes panic from
    /// cancellation so the log doesn't lie about which one occurred.
    async fn handle_generate_thumbs(
        self: &Arc<Self>,
        book_id: i64,
        last_modified_epoch: i64,
    ) -> TaskOutcome {
        let pool = self.pool.clone();
        let cover = match crate::covers::get_cover(&pool, book_id).await {
            Ok(Some((_mime, bytes))) => bytes,
            Ok(None) => {
                return TaskOutcome::Err(format!("no cover for book {book_id}"));
            }
            Err(e) => return TaskOutcome::Err(e.to_string()),
        };
        let cap = crate::thumbs::cap_bytes();
        match tokio::task::spawn_blocking(move || {
            crate::thumbs::ensure_thumbnails_sync(book_id, last_modified_epoch, cover)?;
            crate::thumbs::evict_if_over_cap(cap)
                .map_err(|e| crate::thumbs::ThumbError::Failed(format!("I/O error: {e}")))
        })
        .await
        {
            Ok(Ok(())) => TaskOutcome::Ok,
            Ok(Err(e)) => TaskOutcome::Err(e.to_string()),
            Err(join_err) => {
                let kind = if join_err.is_panic() {
                    "panicked"
                } else {
                    "was cancelled"
                };
                TaskOutcome::Err(format!("spawn_blocking {kind}: {join_err}"))
            }
        }
    }
}

/// Run the test-only synthetic task: fire the optional `on_run` hook, sleep
/// `latency_ms`, then fire the optional `on_done` hook.
#[cfg(test)]
async fn handle_test_task(
    latency_ms: u64,
    on_run: Option<Arc<dyn Fn() + Send + Sync>>,
    on_done: Option<Arc<dyn Fn() + Send + Sync>>,
) -> TaskOutcome {
    if let Some(f) = on_run.as_ref() {
        f();
    }
    tokio::time::sleep(std::time::Duration::from_millis(latency_ms)).await;
    if let Some(f) = on_done.as_ref() {
        f();
    }
    TaskOutcome::Ok
}
