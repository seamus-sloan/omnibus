//! Per-task-kind handlers. `Worker::execute` is the single dispatch
//! match — each arm pulls the inputs out of a [`Task`] variant and calls
//! the owning module (`indexer::reindex`, `author_photos::resolve`,
//! `thumbs::ensure_thumbnails_sync`). Every arm ends in a [`TaskOutcome`]
//! whose `Err` text is client-facing (served by `rpc_worker_status` and
//! the owner-scoped Kindle poll), so no arm may return a raw error's
//! `Display` verbatim — see the sanitizers below.

use std::sync::Arc;

use super::types::{Task, TaskId, TaskOutcome, Worker};

/// Log `e`'s real text server-side and return a [`TaskOutcome::Err`] that
/// names only `label` — never `e` itself. Used for every failure whose
/// `Display` can carry a filesystem path, subprocess stderr, or a
/// lower-level crate's internals (DB driver, SMTP transport), none of
/// which are safe to hand to whoever polls the worker status.
fn sanitized_err(label: &str, e: impl std::fmt::Display) -> TaskOutcome {
    tracing::error!(error = %e, task = label, "worker task failed");
    TaskOutcome::Err(format!("{label} failed; see server logs for details"))
}

/// [`sanitized_err`]-based mapping for the handlers whose failure space is
/// `anyhow` (foreign systems: filesystem scans, ffmpeg, parsers) and thus
/// has no enumerable variants to bucket. The `Ok` payload is discarded
/// (some handlers return a path/id), so `Ok(())` and `Ok(value)` collapse
/// to [`TaskOutcome::Ok(None)`] identically. Scan tasks that need to
/// surface a ghost-count warning (issue #1057) build their `TaskOutcome`
/// directly instead of going through this helper.
fn anyhow_outcome<T>(label: &str, result: anyhow::Result<T>) -> TaskOutcome {
    match result {
        Ok(_) => TaskOutcome::Ok(None),
        Err(e) => sanitized_err(label, e),
    }
}

/// Variant-aware mapping for [`crate::kindle::KindleError`], mirroring
/// `server::backend::kindle::post_smtp_test`'s match: the enumerable,
/// non-sensitive variants pass their own message through; the SMTP/lettre
/// transport variants and I/O get [`sanitized_err`] instead.
fn kindle_outcome(result: Result<(), crate::kindle::KindleError>) -> TaskOutcome {
    use crate::kindle::KindleError;
    match result {
        Ok(()) => TaskOutcome::Ok(None),
        Err(
            e @ (KindleError::NotConfigured
            | KindleError::NoEpub
            | KindleError::TooLarge
            | KindleError::Timeout),
        ) => TaskOutcome::Err(e.to_string()),
        Err(e) => sanitized_err("send to Kindle", e),
    }
}

/// Variant-aware mapping for [`crate::kepub::KepubError`]: a bad book id or
/// a missing source EPUB are safe, specific messages; a non-zero
/// `kepubify` exit carries subprocess stderr, and the remaining variants
/// wrap I/O or a lower module's internals, so all of those go through
/// [`sanitized_err`].
fn kepub_outcome(result: Result<std::path::PathBuf, crate::kepub::KepubError>) -> TaskOutcome {
    use crate::kepub::KepubError;
    match result {
        Ok(_) => TaskOutcome::Ok(None),
        Err(e @ (KepubError::BookNotFound(_) | KepubError::SourceMissing(_))) => {
            TaskOutcome::Err(e.to_string())
        }
        Err(e) => sanitized_err("kepub conversion", e),
    }
}

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
                    Ok(stats) => TaskOutcome::Ok(stats.ghost_warning()),
                    Err(e) => sanitized_err("library scan", e),
                }
            }
            Task::ScanAudiobooks { library_path } => {
                self.handle_scan_audiobooks(library_path, id).await
            }
            Task::HlsTranscode {
                book_id,
                book_file_id,
                library_path,
                profile,
            } => anyhow_outcome(
                "HLS transcode",
                crate::hls::transcode_book(
                    &self.pool,
                    book_id,
                    book_file_id,
                    &library_path,
                    &profile,
                )
                .await,
            ),
            Task::ResolveAuthorPhoto { author_id } => anyhow_outcome(
                "author photo lookup",
                crate::author_photos::resolve(&self.pool, author_id).await,
            ),
            Task::RefetchAuthorPhotos => {
                match crate::author_photos::refetch_all(&self.pool, |processed, total| {
                    self.report_progress(id, processed, total);
                })
                .await
                {
                    Ok(()) => TaskOutcome::Ok(None),
                    Err(e) => sanitized_err("author photo refetch", e),
                }
            }
            Task::BackfillChapters { library_path } => anyhow_outcome(
                "chapter backfill",
                crate::indexer::backfill_chapters(&self.pool, &library_path, |processed, total| {
                    self.report_progress(id, processed, Some(total));
                })
                .await,
            ),
            Task::RebuildFtsIndex => anyhow_outcome(
                "FTS rebuild",
                crate::sync::rebuild_all_fts(&self.pool).await,
            ),
            Task::ResolveSuggestions { book_uuid } => anyhow_outcome(
                "suggestions lookup",
                crate::suggestions::resolve(&self.pool, &book_uuid).await,
            ),
            Task::KepubConvert { book_id } => {
                kepub_outcome(crate::kepub::convert_book(&self.pool, book_id).await)
            }
            Task::SendToKindle {
                book_id,
                book_file_id,
                recipient_email,
            } => kindle_outcome(
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
            Ok(stats) => {
                let warning = stats.ghost_warning();
                self.post(Task::BackfillChapters { library_path });
                TaskOutcome::Ok(warning)
            }
            Err(e) => sanitized_err("audiobook scan", e),
        }
    }

    /// Fetch a book's cover and (re)generate its WebP thumbnails on the
    /// blocking pool, then evict the thumb cache if it's over the cap. A
    /// missing cover is a safe, specific error; every other failure (DB,
    /// codec, I/O, a panicked/cancelled blocking task) goes through
    /// [`sanitized_err`] since its text can carry driver or codec
    /// internals — the real cause is still logged server-side.
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
            Err(e) => return sanitized_err("thumbnail generation", e),
        };
        let cap = crate::thumbs::cap_bytes();
        match tokio::task::spawn_blocking(move || {
            crate::thumbs::ensure_thumbnails_sync(book_id, last_modified_epoch, cover)?;
            crate::thumbs::evict_if_over_cap(cap)
                .map_err(|e| crate::thumbs::ThumbError::Failed(format!("I/O error: {e}")))
        })
        .await
        {
            Ok(Ok(())) => TaskOutcome::Ok(None),
            Ok(Err(crate::thumbs::ThumbError::NoCover(id))) => {
                TaskOutcome::Err(format!("no cover for book {id}"))
            }
            Ok(Err(e)) => sanitized_err("thumbnail generation", e),
            Err(join_err) => {
                let kind = if join_err.is_panic() {
                    "panicked"
                } else {
                    "was cancelled"
                };
                tracing::error!(
                    error = %join_err,
                    task = "thumbnail generation",
                    "worker task join failure"
                );
                TaskOutcome::Err(format!("thumbnail generation {kind}"))
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
    TaskOutcome::Ok(None)
}

#[cfg(test)]
mod tests;
