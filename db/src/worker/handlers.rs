//! Per-task-kind handlers. `Worker::execute` is the single dispatch
//! match — each arm pulls the inputs out of a [`Task`] variant and calls
//! the owning module (`indexer::reindex`, `author_photos::resolve`,
//! `thumbs::ensure_thumbnails_sync`).

use std::sync::Arc;

use super::types::{Task, TaskId, TaskOutcome, Worker};

impl Worker {
    pub(super) async fn execute(self: &Arc<Self>, task: Task, id: TaskId) -> TaskOutcome {
        match task {
            Task::Scan { library_path } => {
                match crate::indexer::reindex_with_progress(
                    &self.pool,
                    &library_path,
                    |processed, total| {
                        self.report_progress(id, processed, Some(total));
                    },
                )
                .await
                {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::ScanAudiobooks { library_path } => {
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
                        // Post a follow-up chapter backfill task now that the
                        // scan has populated book_file_parts rows. Same resource
                        // key means it won't run until this task fully finishes
                        // (including the terminal-state write in `run`), but
                        // the post itself is instant.
                        self.post(Task::BackfillChapters {
                            library_path: library_path.clone(),
                        });
                        TaskOutcome::Ok
                    }
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::HlsTranscode {
                book_id,
                book_file_id,
                library_path,
                profile,
            } => {
                match crate::hls::transcode_book(
                    &self.pool,
                    book_id,
                    book_file_id,
                    &library_path,
                    &profile,
                )
                .await
                {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::ResolveAuthorPhoto { author_id } => {
                match crate::author_photos::resolve(&self.pool, author_id).await {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::RefetchAuthorPhotos => {
                match crate::author_photos::refetch_all(&self.pool, |processed, total| {
                    self.report_progress(id, processed, total);
                })
                .await
                {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::BackfillChapters { library_path } => {
                match crate::indexer::backfill_chapters(
                    &self.pool,
                    &library_path,
                    |processed, total| {
                        self.report_progress(id, processed, Some(total));
                    },
                )
                .await
                {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::RebuildFtsIndex => match crate::sync::rebuild_all_fts(&self.pool).await {
                Ok(()) => TaskOutcome::Ok,
                Err(e) => TaskOutcome::Err(e.to_string()),
            },
            Task::ResolveSuggestions { book_uuid } => {
                match crate::suggestions::resolve(&self.pool, &book_uuid).await {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::KepubConvert { book_id } => {
                match crate::kepub::convert_book(&self.pool, book_id).await {
                    Ok(_) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::SendToKindle {
                book_id,
                book_file_id,
                recipient_email,
            } => {
                match crate::kindle::send(&self.pool, book_id, book_file_id, &recipient_email).await
                {
                    Ok(()) => TaskOutcome::Ok,
                    Err(e) => TaskOutcome::Err(e.to_string()),
                }
            }
            Task::GenerateThumbs {
                book_id,
                last_modified_epoch,
            } => {
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
                        // `JoinError` covers both panics and cancellation —
                        // distinguish so the log doesn't lie about which one.
                        let kind = if join_err.is_panic() {
                            "panicked"
                        } else {
                            "was cancelled"
                        };
                        TaskOutcome::Err(format!("spawn_blocking {kind}: {join_err}"))
                    }
                }
            }
            #[cfg(test)]
            Task::Test {
                tag: _,
                latency_ms,
                on_run,
                on_done,
                ..
            } => {
                if let Some(f) = on_run.as_ref() {
                    f();
                }
                tokio::time::sleep(std::time::Duration::from_millis(latency_ms)).await;
                if let Some(f) = on_done.as_ref() {
                    f();
                }
                TaskOutcome::Ok
            }
        }
    }
}
