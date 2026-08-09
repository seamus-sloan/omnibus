//! Public type surface and shared helpers for the worker.
//!
//! Owns `Task` / `TaskOutcome` / `TaskId` / `WorkerConfig`, the `Worker`
//! struct, `ProgressEntry`, `TERMINAL_RETENTION`, and the `lock_unpoison` /
//! `wall_clock_ms` helpers the sibling modules share.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use omnibus_shared::{TaskKind, TaskProgress};
use sqlx::SqlitePool;
use tokio::sync::{watch, Mutex, Semaphore};

/// How long a terminal ([`omnibus_shared::ProgressState::Done`] /
/// [`omnibus_shared::ProgressState::Failed`]) entry sticks around in
/// [`Worker::progress_snapshot`] after its `terminal_at` timestamp before
/// lazy eviction drops it. Long enough that a 1 Hz polling client always
/// observes the transition; short enough that the in-memory map stays
/// bounded by current concurrency + a handful of recently-finished tasks.
pub(super) const TERMINAL_RETENTION: Duration = Duration::from_secs(10);

/// A unit of background work handed to [`Worker::post`].
///
/// Each variant carries the inputs its handler needs and determines two
/// scheduling properties via the private `resource_key` / `uses_scan_sem`
/// helpers: which per-resource keyed mutex (if any) serializes it against
/// peers, and whether it counts against the scan-concurrency semaphore.
/// See [`Worker`] for how those interact.
///
/// `#[non_exhaustive]` so adding a variant is not a breaking change for
/// downstream `match`es; new variants must wire up both scheduling helpers
/// and the `execute` dispatch arm.
#[non_exhaustive]
pub enum Task {
    /// Reindex the library rooted at `library_path` (full scan → DB upsert
    /// via `indexer::reindex`). Keyed on the path, so two scans of the same
    /// library serialize while different libraries scan in parallel; counts
    /// against the scan-concurrency semaphore.
    Scan { library_path: String },
    /// Audiobook-library sibling of [`Task::Scan`]. Same keying (one
    /// resource lock per path) and same scan-semaphore participation;
    /// routes to [`crate::indexer::reindex_audiobooks`].
    ScanAudiobooks { library_path: String },
    /// (Re)generate cached WebP thumbnails for `book_id`'s cover.
    /// `last_modified_epoch` lets the handler skip work when the cached
    /// thumbnails are already current. Keyed on `thumb:{book_id}` and does
    /// not consume the scan semaphore, so thumbnailing runs alongside scans.
    GenerateThumbs {
        book_id: i64,
        last_modified_epoch: i64,
    },
    /// Resolve and cache an author's profile photo. The resolver hits Open
    /// Library at most once per author per (admin-DELETE-able) cache window;
    /// a `'letter'` marker is written on any miss so future page views skip
    /// the network entirely. Keyed on `author-photo:{author_id}` and does
    /// not consume the scan semaphore.
    ResolveAuthorPhoto { author_id: i64 },
    /// HLS transcode for one `(book_id, profile)` pair. Acquires the HLS
    /// semaphore (capped at [`WorkerConfig::hls_concurrency`]) and the
    /// per-`(book_id, profile)` keyed mutex so duplicate transcode posts are
    /// serialized rather than running concurrently.
    HlsTranscode {
        book_id: i64,
        book_file_id: i64,
        library_path: String,
        profile: String,
    },
    /// Bulk re-resolve all author photos. Clears non-manual cached photos
    /// and re-runs the Open Library cascade for every author. Reports
    /// progress as `(processed, total)` for the UI indicator. Keyed on a
    /// fixed resource so concurrent clicks serialize; does not consume the
    /// scan semaphore.
    RefetchAuthorPhotos,
    /// Backfill `file_chapters` rows for audiobooks that were indexed before
    /// the chapter pipeline existed. Posted by the [`Task::ScanAudiobooks`]
    /// handler on success so it always runs after the scan completes. Also
    /// triggerable manually from the settings page. Keyed on
    /// `audiobooks:{library_path}` (mutual exclusion with scans on the same
    /// library). Does not consume the scan semaphore (lightweight IO).
    BackfillChapters { library_path: String },
    /// Backfill `books.word_count` for EPUB books indexed before the
    /// word-count column existed (migration `0049`). Posted by the
    /// [`Task::Scan`] handler on success so it always runs after the ebook
    /// scan completes. Keyed on `library_path` (mutual exclusion with the
    /// scan on the same library); does not consume the scan semaphore
    /// (light per-file IO, mirrors [`Task::BackfillChapters`]).
    BackfillWordCounts { library_path: String },
    /// Backfill `books.page_count` for CBZ books indexed before the
    /// page-count column existed (migration `0063`, #1593). Posted by the
    /// [`Task::Scan`] handler on success so it always runs after the ebook
    /// scan completes. Keyed on `library_path` (mutual exclusion with the
    /// scan on the same library); does not consume the scan semaphore
    /// (light per-file IO, mirrors [`Task::BackfillWordCounts`]).
    BackfillPageCounts { library_path: String },
    /// Pre-generate WebP thumbnails (all three sizes) for every covered book
    /// under `library_path` (#1752). Posted by the [`Task::Scan`] handler on
    /// success, alongside [`Task::BackfillWordCounts`] /
    /// [`Task::BackfillPageCounts`], so the landing grid's first post-scan
    /// load serves cached thumbnails instead of falling through the lazy
    /// generation path in `server::backend::covers::thumb_cache_miss_response`.
    /// Keyed on `library_path` (mutual exclusion with the scan on the same
    /// library, and with the other scan-follow-up backfills); does not
    /// consume the scan semaphore. Skips any book whose three sizes are
    /// already fresh, so a re-scan of an unchanged library does no
    /// re-encoding.
    BackfillThumbs { library_path: String },
    /// Rebuild the entire `books_fts` search index from `books` via
    /// `crate::sync::rebuild_all_fts`. Admin-triggered repair for any drift
    /// left by a failed post-commit FTS refresh. Keyed on a fixed resource
    /// so concurrent clicks serialize; does not consume the scan semaphore.
    RebuildFtsIndex,
    /// Resolve and cache "readers also enjoyed" suggestions for one book
    /// via Hardcover list co-occurrence. Keyed on `suggestions:{book_uuid}` so
    /// duplicate posts for one book (a burst of viewers) serialize and the
    /// later ones no-op against the fresh cache; does not consume the scan
    /// semaphore.
    ResolveSuggestions { book_uuid: String },
    /// Convert one book's EPUB to a cached KEPUB (Kobo sideload download).
    /// Keyed on `kepub:{book_id}` so a burst of first-time downloads for one
    /// book collapses onto a single kepubify run; does not consume the scan
    /// semaphore (light single-file work).
    KepubConvert { book_id: i64 },
    /// Email a book's EPUB to a user's Kindle address over SMTP. Keyed on
    /// a fixed `smtp` resource so every send serializes against the single
    /// configured relay (one slow SMTP server can't fan out); does not consume
    /// the scan semaphore. `book_file_id` targets a specific `book_files` row
    /// for multi-EPUB books, else the book's default EPUB.
    SendToKindle {
        book_id: i64,
        book_file_id: Option<i64>,
        recipient_email: String,
    },
    /// Bake every book's active metadata/cover override into its EPUB
    /// container in one pass (#959, #1718), dispatched off the request
    /// thread so the admin's "Bake Overrides Into EPUBs" action returns as
    /// soon as the run is queued instead of awaiting it inline. Keyed on a
    /// fixed resource so concurrent clicks serialize; does not consume the
    /// scan semaphore (its DB work is a handful of bulk-fetched queries, not
    /// per-book round trips).
    RewriteAllEpubs,
    /// Test-only synthetic task: sleeps `latency_ms` and invokes the
    /// optional `on_run` / `on_done` hooks, with `resource` and
    /// `route_through_scan_sem` letting a test exercise the keyed mutex and
    /// scan semaphore directly. Compiled out of non-test builds.
    #[cfg(test)]
    Test {
        tag: &'static str,
        latency_ms: u64,
        resource: Option<String>,
        route_through_scan_sem: bool,
        on_run: Option<Arc<dyn Fn() + Send + Sync>>,
        on_done: Option<Arc<dyn Fn() + Send + Sync>>,
    },
}

impl Task {
    pub(super) fn resource_key(&self) -> Option<String> {
        match self {
            Task::Scan { library_path } => Some(library_path.clone()),
            Task::ScanAudiobooks { library_path } => Some(format!("audiobooks:{library_path}")),
            Task::GenerateThumbs { book_id, .. } => Some(format!("thumb:{book_id}")),
            Task::ResolveAuthorPhoto { author_id } => Some(format!("author-photo:{author_id}")),
            Task::HlsTranscode {
                book_id, profile, ..
            } => Some(format!("hls:{book_id}:{profile}")),
            Task::RefetchAuthorPhotos => Some("refetch-author-photos".into()),
            Task::BackfillChapters { library_path } => Some(format!("audiobooks:{library_path}")),
            Task::BackfillWordCounts { library_path } => Some(library_path.clone()),
            Task::BackfillPageCounts { library_path } => Some(library_path.clone()),
            Task::BackfillThumbs { library_path } => Some(library_path.clone()),
            Task::RebuildFtsIndex => Some("rebuild-fts".into()),
            Task::ResolveSuggestions { book_uuid } => Some(format!("suggestions:{book_uuid}")),
            Task::KepubConvert { book_id } => Some(format!("kepub:{book_id}")),
            Task::SendToKindle { .. } => Some("smtp".into()),
            Task::RewriteAllEpubs => Some("rewrite-all-epubs".into()),
            #[cfg(test)]
            Task::Test { resource, .. } => resource.clone(),
        }
    }

    pub(super) fn uses_scan_sem(&self) -> bool {
        match self {
            Task::Scan { .. } => true,
            Task::ScanAudiobooks { .. } => true,
            Task::GenerateThumbs { .. } => false,
            Task::ResolveAuthorPhoto { .. } => false,
            Task::HlsTranscode { .. } => false,
            Task::RefetchAuthorPhotos => false,
            Task::BackfillChapters { .. } => false,
            Task::BackfillWordCounts { .. } => false,
            Task::BackfillPageCounts { .. } => false,
            Task::BackfillThumbs { .. } => false,
            Task::RebuildFtsIndex => false,
            Task::ResolveSuggestions { .. } => false,
            Task::KepubConvert { .. } => false,
            Task::SendToKindle { .. } => false,
            Task::RewriteAllEpubs => false,
            #[cfg(test)]
            Task::Test {
                route_through_scan_sem,
                ..
            } => *route_through_scan_sem,
        }
    }

    /// `true` for tasks that should acquire the HLS concurrency semaphore.
    pub(super) fn uses_hls_sem(&self) -> bool {
        matches!(self, Task::HlsTranscode { .. })
    }

    /// Free-text label persisted to the `background_tasks` table (issue
    /// #941, migration `0070`). Deliberately finer-grained than
    /// [`Task::kind`]'s wire-facing [`TaskKind`] — several `Task` variants
    /// share one `TaskKind` for the live progress UI (see that method's
    /// per-arm comments), but the admin history view wants to tell e.g. a
    /// KEPUB conversion apart from a full library scan.
    pub(super) fn persistence_kind(&self) -> &'static str {
        match self {
            Task::Scan { .. } => "scan",
            Task::ScanAudiobooks { .. } => "scan_audiobooks",
            Task::GenerateThumbs { .. } => "generate_thumbs",
            Task::ResolveAuthorPhoto { .. } => "resolve_author_photo",
            Task::HlsTranscode { .. } => "hls_transcode",
            Task::RefetchAuthorPhotos => "refetch_author_photos",
            Task::BackfillChapters { .. } => "backfill_chapters",
            Task::BackfillWordCounts { .. } => "backfill_word_counts",
            Task::BackfillPageCounts { .. } => "backfill_page_counts",
            Task::BackfillThumbs { .. } => "backfill_thumbs",
            Task::RebuildFtsIndex => "rebuild_fts_index",
            Task::ResolveSuggestions { .. } => "resolve_suggestions",
            Task::KepubConvert { .. } => "kepub_convert",
            Task::SendToKindle { .. } => "send_to_kindle",
            Task::RewriteAllEpubs => "rewrite_all_epubs",
            #[cfg(test)]
            Task::Test { .. } => "test",
        }
    }

    /// Wire-protocol discriminant exposed to the UI via
    /// [`Worker::progress_snapshot`]. Tests deliberately collapse onto an
    /// existing variant so the [`TaskKind`] enum doesn't grow a `Test` arm
    /// that downstream UIs would have to render.
    pub(super) fn kind(&self) -> TaskKind {
        match self {
            Task::Scan { .. } => TaskKind::Scan,
            Task::ScanAudiobooks { .. } => TaskKind::Scan,
            Task::GenerateThumbs { .. } => TaskKind::GenerateThumbs,
            Task::ResolveAuthorPhoto { .. } => TaskKind::ResolveAuthorPhoto,
            Task::RefetchAuthorPhotos => TaskKind::RefetchAuthorPhotos,
            Task::BackfillChapters { .. } => TaskKind::BackfillChapters,
            // Reuse Scan kind — a scan-follow-up with no dedicated progress
            // widget, mirroring HLS/FTS/KEPUB below.
            Task::BackfillWordCounts { .. } => TaskKind::Scan,
            // Same reuse as BackfillWordCounts, its sibling scan-follow-up.
            Task::BackfillPageCounts { .. } => TaskKind::Scan,
            // Reuse the per-book GenerateThumbs kind rather than Scan: unlike
            // its sibling backfills, this one has an existing, sensible
            // "Generating thumbnail" / "Thumbnail generation" label already
            // wired in `frontend::components::worker_status` (AC4).
            Task::BackfillThumbs { .. } => TaskKind::GenerateThumbs,
            // Reuse Scan kind for UI display until a dedicated HLS progress
            // widget is added.
            Task::HlsTranscode { .. } => TaskKind::Scan,
            // Reuse Scan kind for the FTS rebuild's progress display rather
            // than growing the wire-facing `TaskKind` enum for a rare admin job.
            Task::RebuildFtsIndex => TaskKind::Scan,
            Task::ResolveSuggestions { .. } => TaskKind::ResolveSuggestions,
            // Reuse Scan kind for the KEPUB conversion's progress display
            // rather than growing the wire-facing `TaskKind` enum.
            Task::KepubConvert { .. } => TaskKind::Scan,
            // Reuse Scan kind for UI display — a send is a rare, short job with
            // no dedicated progress widget (mirrors HLS/FTS).
            Task::SendToKindle { .. } => TaskKind::Scan,
            // Reuse Scan kind for UI display — a rare admin job with no
            // dedicated progress widget, mirroring RebuildFtsIndex/KepubConvert.
            Task::RewriteAllEpubs => TaskKind::Scan,
            #[cfg(test)]
            Task::Test { .. } => TaskKind::Scan,
        }
    }
}

/// Process-local handle returned by [`Worker::post`], used to look up a
/// task's completion via [`Worker::await_completion`]. Monotonically
/// assigned per `Worker`; not stable across restarts and not a DB id.
pub type TaskId = u64;

/// Extra data a successful task attaches to its [`TaskOutcome::Ok`], beyond
/// the bare fact that it succeeded. Each task kind produces at most one of
/// these — they're never combined — and most task kinds produce none at all,
/// which is why [`TaskOutcome::Ok`] wraps this in an `Option`.
#[derive(Clone, Debug, PartialEq)]
pub enum TaskSuccessDetail {
    /// A scan whose ghost count cleared the warn threshold (issue #1057).
    GhostFiles(omnibus_shared::GhostFilesWarning),
    /// A fleet-wide EPUB override bake's failed `book_uuid`s (#1718, #1739).
    /// Only attached when non-empty — a bake where every book succeeded
    /// reports `None` like any other task. Deliberately just the uuids, not
    /// the per-book error text (which can carry a server filesystem path) —
    /// see the doc comment on `omnibus_shared::ProgressState::Done::bake_errors`.
    BakeErrors(Vec<String>),
}

/// Terminal result of a task, delivered to awaiters of its [`TaskId`].
#[derive(Clone, Debug)]
pub enum TaskOutcome {
    /// The handler ran to completion successfully. `Some(_)` only for the
    /// task kinds that attach a [`TaskSuccessDetail`] (a scan whose ghost
    /// count cleared the warn threshold, or a bake that left per-book
    /// errors); every other task kind (and those two under their
    /// respective all-clear condition) reports `None`.
    Ok(Option<TaskSuccessDetail>),
    /// The handler failed. This string is client-facing (served by
    /// `rpc_worker_status` and the owner-scoped Kindle poll), so
    /// `handlers::execute`'s arms sanitize it before it lands here — never
    /// the raw underlying error's `Display`. Also produced when the
    /// spawned task is dropped or panics before reporting (see
    /// [`Worker::await_completion`]).
    Err(String),
}

/// Construction-time tuning for a [`Worker`].
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Maximum number of scan-semaphore tasks (currently [`Task::Scan`] /
    /// [`Task::ScanAudiobooks`]) allowed to run concurrently. Clamped to at
    /// least 1 by [`Worker::new`]. Other task types are unaffected by this cap.
    pub scan_concurrency: usize,
    /// Maximum number of concurrent [`Task::HlsTranscode`] jobs. Each
    /// transcode drives one ffmpeg process; defaults to
    /// `max(1, num_cpus / 2)` so a two-core machine runs one transcode at
    /// a time while an eight-core machine can run four concurrently.
    pub hls_concurrency: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        let cpus = num_cpus();
        Self {
            scan_concurrency: 1,
            hls_concurrency: (cpus / 2).max(1),
        }
    }
}

/// Number of logical CPUs. Reads `/proc/cpuinfo` via `std::thread` so we
/// avoid adding a `num_cpus` crate for one call site.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1)
}

/// Single-process background-task runner shared behind an `Arc`.
///
/// Posting a [`Task`] spawns it on the tokio runtime and returns a
/// [`TaskId`] immediately ([`post`](Worker::post) returns as soon as the
/// task is spawned; it does not wait for the task to run to completion).
/// Two fairness mechanisms shape execution:
///
/// * **Per-resource keyed mutex** — tasks reporting the same resource key
///   (e.g. two scans of the same library path) serialize behind one
///   another, while tasks on different keys run concurrently.
/// * **Scan-concurrency semaphore** — scan-class tasks additionally
///   contend for a fixed pool of permits sized by
///   [`WorkerConfig::scan_concurrency`], capping how many run at once
///   regardless of resource key.
///
/// The resource lock is always acquired before the scan permit so a task
/// queued behind a same-resource peer never holds a permit while idle.
/// Owns the [`SqlitePool`] its handlers run against.
pub struct Worker {
    pub(super) pool: SqlitePool,
    pub(super) scan_sem: Arc<Semaphore>,
    /// Semaphore capping concurrent `Task::HlsTranscode` runs. Separate from
    /// `scan_sem` so HLS jobs don't compete with library scans for permits.
    pub(super) hls_sem: Arc<Semaphore>,
    pub(super) resource_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    pub(super) completions: Arc<StdMutex<HashMap<TaskId, watch::Receiver<Option<TaskOutcome>>>>>,
    /// Live snapshot of every posted task's lifecycle state. Holds entries
    /// from `post` until ~[`TERMINAL_RETENTION`] after the task reaches a
    /// terminal state. Read path is [`Worker::progress_snapshot`]; write
    /// paths are `post` (initial Running) and `run` (terminal Done/Failed,
    /// plus the panic-safety guard).
    pub(super) progress: Arc<StdMutex<BTreeMap<TaskId, ProgressEntry>>>,
    /// Bounded per-[`TaskKind`] window of recent completion durations,
    /// backing [`Worker::metrics`]. Written once per terminal transition by
    /// [`super::progress::write_terminal_progress`]; capped at
    /// [`super::metrics::RECENT_COMPLETIONS_CAP`] entries per kind. A
    /// separate `StdMutex` from `progress`, so recording a completion
    /// releases the `progress` lock first rather than extending its hold
    /// time — `Worker::metrics` still takes `progress` separately to
    /// compute queue depth.
    pub(super) completion_timings: Arc<StdMutex<HashMap<TaskKind, VecDeque<Duration>>>>,
    pub(super) next_id: std::sync::atomic::AtomicU64,
}

/// Internal pairing of the wire-facing [`TaskProgress`] with a monotonic
/// `terminal_at` timestamp used purely for eviction. We intentionally don't
/// reuse `TaskProgress.last_update_ms` (wall-clock; can jump under NTP) for
/// expiry decisions — that field exists only so the UI can render elapsed
/// time.
pub(super) struct ProgressEntry {
    pub(super) progress: TaskProgress,
    pub(super) terminal_at: Option<Instant>,
    /// User who owns this task, for user-initiated pollable jobs (e.g.
    /// Send-to-Kindle). `None` for system tasks (scans, transcodes). Gates
    /// [`Worker::owned_task_state`] so the monotonic, guessable task-id space
    /// can't be probed across users. Evicted with the entry.
    pub(super) owner: Option<i64>,
}

/// Recover from a poisoned `std::sync::Mutex` instead of panicking.
///
/// The `completions` / `progress` maps are best-effort bookkeeping that
/// every `Task::Scan` (and every other posted task) touches on its hot
/// path. If a thread panics while holding one of these locks — e.g. a
/// `ProgressTerminalGuard::drop` unwinding mid-write — `std::sync::Mutex`
/// poisons permanently, and a plain `.lock().unwrap()` would then turn that
/// one-off panic into a process-wide crash on every subsequent task. The
/// guarded data is just in-memory maps with no invariant left broken by a
/// partial write, so taking the inner guard and carrying on is strictly
/// safer than tearing down the worker. Mirrors `frontend::data`'s
/// `unpoison` helper.
pub(super) fn lock_unpoison<T>(m: &StdMutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Current wall-clock time in milliseconds since the UNIX epoch. Used only
/// for the `started_at_ms` / `last_update_ms` fields on [`TaskProgress`],
/// which the UI renders as elapsed-time hints. A backward NTP step would
/// produce a wonky elapsed-time display for one polling tick — well within
/// the UI's tolerance.
pub(super) fn wall_clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Current wall-clock time in whole seconds since the UNIX epoch — the
/// granularity `background_tasks.started_at`/`finished_at` (migration
/// `0070`) store. Derived from [`wall_clock_ms`] rather than an independent
/// `SystemTime` read, so the two can never disagree.
pub(super) fn wall_clock_secs() -> i64 {
    wall_clock_ms() / 1000
}

impl Worker {
    /// Build a `Worker` over `pool` with the given `config`, returning it
    /// behind an `Arc` (every method takes `&Arc<Self>` or `&self`, and
    /// posted tasks clone the `Arc` into their spawned future).
    pub fn new(pool: SqlitePool, config: WorkerConfig) -> Arc<Self> {
        Arc::new(Self {
            pool,
            scan_sem: Arc::new(Semaphore::new(config.scan_concurrency.max(1))),
            hls_sem: Arc::new(Semaphore::new(config.hls_concurrency.max(1))),
            resource_locks: Arc::new(Mutex::new(HashMap::new())),
            completions: Arc::new(StdMutex::new(HashMap::new())),
            progress: Arc::new(StdMutex::new(BTreeMap::new())),
            completion_timings: Arc::new(StdMutex::new(HashMap::new())),
            next_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    #[cfg(test)]
    pub(super) fn completions_len(&self) -> usize {
        self.completions.lock().unwrap().len()
    }

    #[cfg(test)]
    pub(super) async fn resource_locks_len(&self) -> usize {
        self.resource_locks.lock().await.len()
    }

    #[cfg(test)]
    pub(super) fn progress_len(&self) -> usize {
        self.progress.lock().unwrap().len()
    }
}
