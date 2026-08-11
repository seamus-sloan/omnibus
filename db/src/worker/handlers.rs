//! Per-task-kind handlers. `Worker::execute`'s single dispatch match pulls
//! each [`Task`] variant's inputs and calls the owning module. Every arm's
//! [`TaskOutcome::Err`] text is client-facing (served by `rpc_worker_status`
//! and the owner-scoped Kindle poll) — no arm may return a raw error's
//! `Display` verbatim; use the sanitizers below.

use std::sync::Arc;

use omnibus_shared::TaskDetail;

use super::types::{Task, TaskId, TaskOutcome, TaskSuccessDetail, Worker};

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
/// a missing source EPUB are safe, specific messages; the collapsed
/// `Failed` variant (DB lookups, I/O, a non-zero kepubify exit — all
/// foreign-system failures) carries subprocess stderr / lower-level
/// internals, so it goes through [`sanitized_err`].
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

/// Variant-aware mapping for [`crate::convert::ConvertError`]: a missing
/// source file, an unavailable `ebook-convert` binary, a timeout, and an
/// invalid format token are safe, specific messages (#948); the collapsed
/// `Failed` variant (DB lookups, I/O, a non-zero `ebook-convert` exit)
/// carries subprocess stderr / lower-level internals, so it goes through
/// [`sanitized_err`].
fn convert_outcome(
    result: Result<std::path::PathBuf, crate::convert::ConvertError>,
) -> TaskOutcome {
    use crate::convert::ConvertError;
    match result {
        Ok(_) => TaskOutcome::Ok(None),
        Err(
            e @ (ConvertError::SourceMissing { .. }
            | ConvertError::BinaryUnavailable
            | ConvertError::Timeout(_)
            | ConvertError::InvalidFormat { .. }),
        ) => TaskOutcome::Err(e.to_string()),
        Err(e) => sanitized_err("format conversion", e),
    }
}

impl Worker {
    pub(super) async fn execute(self: &Arc<Self>, task: Task, id: TaskId) -> TaskOutcome {
        match task {
            Task::Scan { library_path } => self.handle_scan(library_path, id).await,
            Task::ScanAudiobooks { library_path } => {
                self.handle_scan_audiobooks(library_path, id).await
            }
            Task::HlsTranscode {
                book_id,
                book_file_id,
                library_path,
                profile,
            } => {
                self.report_book_detail(id, book_id).await;
                anyhow_outcome(
                    "HLS transcode",
                    crate::hls::transcode_book(
                        &self.pool,
                        book_id,
                        book_file_id,
                        &library_path,
                        &profile,
                    )
                    .await,
                )
            }
            Task::ResolveAuthorPhoto { author_id } => {
                self.report_author_detail(id, author_id).await;
                anyhow_outcome(
                    "author photo lookup",
                    crate::author_photos::resolve(&self.pool, author_id).await,
                )
            }
            Task::RefetchAuthorPhotos => self.handle_refetch_author_photos(id).await,
            Task::BackfillChapters { library_path } => anyhow_outcome(
                "chapter backfill",
                crate::indexer::backfill_chapters(
                    &self.pool,
                    &library_path,
                    |processed, total, item| {
                        self.report_item_progress(id, processed, total, item);
                    },
                )
                .await,
            ),
            Task::BackfillWordCounts { library_path } => anyhow_outcome(
                "word-count backfill",
                crate::indexer::backfill_word_counts(
                    &self.pool,
                    &library_path,
                    |processed, total, item| {
                        self.report_item_progress(id, processed, total, item);
                    },
                )
                .await,
            ),
            Task::BackfillPageCounts { library_path } => anyhow_outcome(
                "page-count backfill",
                crate::indexer::backfill_page_counts(
                    &self.pool,
                    &library_path,
                    |processed, total, item| {
                        self.report_item_progress(id, processed, total, item);
                    },
                )
                .await,
            ),
            Task::BackfillThumbs { library_path } => anyhow_outcome(
                "thumbnail backfill",
                crate::indexer::backfill_thumbs(
                    &self.pool,
                    &library_path,
                    |processed, total, item| {
                        self.report_item_progress(id, processed, total, item);
                    },
                )
                .await,
            ),
            Task::RebuildFtsIndex => anyhow_outcome(
                "FTS rebuild",
                crate::sync::rebuild_all_fts(&self.pool).await,
            ),
            Task::ResolveSuggestions { book_uuid } => {
                self.report_book_detail_by_uuid(id, &book_uuid).await;
                anyhow_outcome(
                    "suggestions lookup",
                    crate::suggestions::resolve(&self.pool, &book_uuid).await,
                )
            }
            Task::KepubConvert { book_id } => {
                self.report_book_detail(id, book_id).await;
                self.handle_kepub_convert(book_id).await
            }
            Task::ConvertFormat {
                book_id,
                source_format,
                target_format,
            } => {
                self.report_book_detail(id, book_id).await;
                convert_outcome(
                    crate::convert::convert_book(
                        &self.pool,
                        book_id,
                        &source_format,
                        &target_format,
                    )
                    .await,
                )
            }
            Task::SendToKindle {
                book_id,
                book_file_id,
                recipient_email,
            } => {
                self.report_book_detail(id, book_id).await;
                kindle_outcome(
                    crate::kindle::send(&self.pool, book_id, book_file_id, &recipient_email).await,
                )
            }
            Task::RewriteAllEpubs => self.handle_rewrite_all_epubs(id).await,
            Task::GenerateThumbs {
                book_id,
                last_modified_epoch,
            } => {
                self.report_book_detail(id, book_id).await;
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

    /// Best-effort current-item report: the book's display title. A lookup
    /// failure (or unknown id) just leaves the task's kind label in place —
    /// naming the item is never worth failing the task over.
    async fn report_book_detail(&self, id: TaskId, book_id: i64) {
        if let Ok(Some(title)) = crate::books::book_display_title(&self.pool, book_id).await {
            self.report_detail(
                id,
                TaskDetail {
                    current_item: Some(title),
                    ..TaskDetail::default()
                },
            );
        }
    }

    /// [`Worker::report_book_detail`] keyed by uuid (merged-uuid aware).
    async fn report_book_detail_by_uuid(&self, id: TaskId, uuid: &str) {
        if let Ok(Some(title)) = crate::books::book_display_title_by_uuid(&self.pool, uuid).await {
            self.report_detail(
                id,
                TaskDetail {
                    current_item: Some(title),
                    ..TaskDetail::default()
                },
            );
        }
    }

    /// Best-effort current-item report: the author's name.
    async fn report_author_detail(&self, id: TaskId, author_id: i64) {
        if let Ok(Some(name)) = crate::author_photos_data::author_name(&self.pool, author_id).await
        {
            self.report_detail(
                id,
                TaskDetail {
                    current_item: Some(name),
                    ..TaskDetail::default()
                },
            );
        }
    }

    /// Counted progress plus the current item in one report — the shape the
    /// per-book backfill and bake loops feed.
    fn report_item_progress(&self, id: TaskId, processed: u32, total: u32, item: &str) {
        self.report_progress_update(
            id,
            processed,
            Some(total),
            TaskDetail {
                current_item: Some(item.to_string()),
                ..TaskDetail::default()
            },
        );
    }

    /// `Task::KepubConvert` plus the Web→Kobo annotation materialization it
    /// unblocks. A KoboSpan anchor is derivable only against the converted
    /// KEPUB, so a highlight written while the cache was cold is left
    /// unresolved by `downsync_book_annotations` — which requests exactly this
    /// task on its way out. Running the pass here is what makes that request
    /// pay off: without it the row waits for the next highlight write or the
    /// next boot, because an unmaterialized row never moves the wire
    /// fingerprint that `checkforchanges` uses to invite a re-pull.
    /// Non-fatal — the conversion itself is the task's contract.
    async fn handle_kepub_convert(self: &Arc<Self>, book_id: i64) -> TaskOutcome {
        let outcome = kepub_outcome(crate::kepub::convert_book(&self.pool, book_id).await);
        if matches!(outcome, TaskOutcome::Ok(_)) {
            if let Err(e) =
                crate::annotations::downsync_book_id_annotations(&self.pool, book_id).await
            {
                tracing::warn!(book_id, error = %e, "post-conversion annotation downsync failed");
            }
        }
        outcome
    }

    /// Reindex an ebook library, then (on success) post the follow-up
    /// word-count, page-count, and thumbnail-warm-up (#1752) backfill tasks
    /// for any rows this library still needs them for. All three share the
    /// scan's resource key, so each waits for this task to fully finish; the
    /// posts themselves are instant. Mirrors the audiobook chapter backfill.
    async fn handle_scan(self: &Arc<Self>, library_path: String, id: TaskId) -> TaskOutcome {
        // Owned clone: the verbose callback rides into the indexer's
        // blocking parse phase, which needs `Send + 'static`.
        let worker = self.clone();
        match crate::indexer::reindex_with_progress(&self.pool, &library_path, move |u| {
            worker.report_progress_update(id, u.processed, u.total, u.detail);
        })
        .await
        {
            Ok(stats) => {
                let warning = stats.ghost_warning();
                self.post(Task::BackfillWordCounts {
                    library_path: library_path.clone(),
                });
                self.post(Task::BackfillPageCounts {
                    library_path: library_path.clone(),
                });
                self.post(Task::BackfillThumbs { library_path });
                TaskOutcome::Ok(warning.map(TaskSuccessDetail::GhostFiles))
            }
            Err(e) => sanitized_err("library scan", e),
        }
    }

    /// Bake every book's active override into its EPUB export cache
    /// (#959, #1718). Per-book failures are recorded in
    /// [`omnibus_shared::BulkRewriteSummary::errors`] by
    /// `rewrite_all_epubs_with_overrides` itself and never fail the task —
    /// only a failure resolving the batch (DB down, etc.) reaches this
    /// match's `Err` arm. A non-empty error list rides onto the terminal
    /// `TaskOutcome` as [`TaskSuccessDetail::BakeErrors`] (#1739) so the
    /// poller can surface it, in addition to the summary counts always
    /// being logged. Only the failed `book_uuid`s cross that boundary —
    /// each per-book `message` (from `db::epub_rewrite`, which can carry a
    /// server filesystem path) is logged here, server-side only, and never
    /// reaches `rpc_worker_status`, which any authenticated user can poll.
    async fn handle_rewrite_all_epubs(self: &Arc<Self>, id: TaskId) -> TaskOutcome {
        match crate::rewrite_all_epubs_with_progress(&self.pool, |processed, total, current| {
            self.report_item_progress(id, processed, total, current.unwrap_or("…"));
        })
        .await
        {
            Ok(summary) => {
                tracing::info!(
                    rewritten = summary.rewritten,
                    skipped = summary.skipped,
                    errors = summary.errors.len(),
                    "epub override bake finished"
                );
                for err in &summary.errors {
                    tracing::warn!(
                        book_uuid = %err.book_uuid,
                        error = %err.message,
                        "epub override bake failed for book"
                    );
                }
                let detail = (!summary.errors.is_empty()).then(|| {
                    TaskSuccessDetail::BakeErrors(
                        summary.errors.into_iter().map(|e| e.book_uuid).collect(),
                    )
                });
                TaskOutcome::Ok(detail)
            }
            Err(e) => sanitized_err("epub override bake", e),
        }
    }

    /// Re-resolve every author's photo, reporting progress as it goes.
    async fn handle_refetch_author_photos(self: &Arc<Self>, id: TaskId) -> TaskOutcome {
        match crate::author_photos::refetch_all(&self.pool, |processed, total, name| {
            self.report_progress_update(
                id,
                processed,
                total,
                TaskDetail {
                    current_item: name.map(str::to_string),
                    ..TaskDetail::default()
                },
            );
        })
        .await
        {
            Ok(()) => TaskOutcome::Ok(None),
            Err(e) => sanitized_err("author photo refetch", e),
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
        let worker = self.clone();
        match crate::indexer::reindex_audiobooks_with_progress(&self.pool, &library_path, {
            move |u| {
                worker.report_progress_update(id, u.processed, u.total, u.detail);
            }
        })
        .await
        {
            Ok(stats) => {
                let warning = stats.ghost_warning();
                self.post(Task::BackfillChapters { library_path });
                TaskOutcome::Ok(warning.map(TaskSuccessDetail::GhostFiles))
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
