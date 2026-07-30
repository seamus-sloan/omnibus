//! "Update available" detection and its remedy. [`is_stale`]/[`note_book`]
//! answer the question per book immediately; [`refresh_stale_flags`] answers
//! it for every completed download in bulk, on a TTL, via
//! `POST /api/downloads/validators`, so Downloads learns of a replaced file
//! without the reader opening each book. [`redownload`] is the remedy.

use super::{bump, get_entry, registry, set_error, start, target_file, upsert};
use super::{engine, media, store, sync};
use super::{DlFormat, DownloadStatus};

/// Replace a completed download with the library's current bytes.
///
/// **Not** `remove` followed by `start`. `remove` deletes asynchronously, so
/// pairing them races the deletion against the engine writing the
/// replacement into the same directory — and, race or no race, it throws
/// away a book the reader could still open the moment before any network,
/// auth or integrity failure. A stale copy is worth strictly more than no
/// copy.
///
/// Instead the existing files stay exactly where they are. The engine
/// fetches into `{rel}.part` beside them and renames over the top on
/// success, which is atomic per file; if anything fails first, the prior
/// entry is restored and the reader still has the book they had — still
/// flagged, so the chip invites another try.
pub fn redownload(
    server_url: String,
    uuid: String,
    format: DlFormat,
    file_id: Option<i64>,
    title: String,
) {
    let Some(prior) = get_entry(&uuid, format) else {
        // Nothing on disk to protect — an ordinary first download.
        start(server_url, uuid, format, file_id, title);
        return;
    };
    if !matches!(prior.status, DownloadStatus::Complete { .. }) {
        start(server_url, uuid, format, file_id, title);
        return;
    }
    if sync::is_offline() {
        return;
    }

    // "Re-download what I have" means the same file, so a caller that
    // doesn't track the selection (the book-detail row) inherits it rather
    // than silently retargeting a two-edition book at the default row.
    let file_id = file_id.or(prior.file_id);

    // Clear the per-file completion so the engine refetches every file
    // rather than carrying the old bytes forward, while leaving those bytes
    // on disk as the fallback.
    let mut refetching = prior.clone();
    refetching.status = DownloadStatus::Downloading {
        downloaded: 0,
        total: None,
    };
    refetching.title = title;
    refetching.file_id = file_id;
    for file in &mut refetching.files {
        file.done = false;
        file.bytes = None;
    }
    refetching.updated_at = store::now_secs();
    upsert(refetching);

    let spawned = media::spawn_on_runtime(engine::run_replacing(
        server_url,
        uuid.clone(),
        format,
        file_id,
        prior,
    ));
    if !spawned {
        set_error(&uuid, format, "Offline storage unavailable".into());
    }
}

/// Recompute the "Update available" flag for every downloaded format of
/// `book` from freshly-read metadata.
///
/// Called wherever a full [`omnibus_shared::EbookMetadata`] lands — opening
/// a book detail page — so a device learns its copy is stale from a refresh
/// it was making anyway. Only the per-book read carries `book_files`; the
/// landing/replica projection does not, which [`staleness`] answers as
/// "can't tell" and this leaves alone.
pub fn note_book(book: &omnibus_shared::EbookMetadata) {
    let Some(uuid) = book.unique_identifier.as_deref() else {
        return;
    };
    let mut changed = false;
    for format in [DlFormat::Epub, DlFormat::Audio] {
        // `None` means this metadata couldn't answer the question — a
        // projection with no `book_files`, or a row with no validator on
        // one side. Leave the flag as it stands rather than let "can't
        // tell" silently clear a chip a real comparison established.
        let Some(stale) = staleness(uuid, format, book) else {
            continue;
        };
        if let Ok(mut guard) = registry().write() {
            if let Some(entry) = guard.get_mut(&(uuid.to_string(), format)) {
                if entry.stale != stale {
                    entry.stale = stale;
                    changed = true;
                }
            }
        }
    }
    if changed {
        bump();
    }
}

/// How long a staleness answer stays good enough. A file changing on the
/// server is not urgent, and the alternative — asking on every 60-second
/// tick — is a data and battery cost that grows with the reader's library.
/// Opening a book still refreshes that book immediately via [`note_book`].
const STALE_CHECK_TTL_SECS: i64 = 900;

/// When the last validator sweep ran. In memory, so the first tick after a
/// launch always asks: a device that was off for a week should not wait out
/// a TTL it slept through.
pub(super) fn last_stale_check() -> &'static std::sync::atomic::AtomicI64 {
    static AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    &AT
}

/// Refresh the "Update available" flags for every completed download, in as
/// few requests as the server's per-request cap allows.
///
/// Driven by the online background tick, so the Downloads list learns about
/// a replaced file without the reader opening each book. Two properties are
/// deliberate:
///
/// - It asks `POST /api/downloads/validators`, not N metadata fetches. A
///   device with fifty downloads was otherwise making fifty full-book
///   requests every minute.
/// - It writes **no cache rows**. The response carries validators and
///   nothing else, so there is no metadata to smuggle past
///   `cache::read_through`'s compare/put/notify path — which, bypassed,
///   leaves an open page rendering fields the cache has already replaced.
pub(crate) async fn refresh_stale_flags(server_url: &str) {
    use std::sync::atomic::Ordering;

    let now = store::now_secs();
    let last = last_stale_check().load(Ordering::Relaxed);
    if last != 0 && now - last < STALE_CHECK_TTL_SECS {
        return;
    }

    let queries = completed_validator_queries();
    if queries.is_empty() {
        last_stale_check().store(now, Ordering::Relaxed);
        return;
    }
    if sweep_validators(server_url, &queries).await {
        last_stale_check().store(now, Ordering::Relaxed);
    }
    // A chunk that failed leaves the timestamp alone so the next tick
    // retries the whole sweep, rather than waiting out a TTL on a sweep
    // that never finished.
}

/// Ask about every completed download in chunks of at most
/// `MAX_VALIDATOR_QUERY` — the server rejects a single request over that
/// size with `422` (`post_download_validators`), and a device can easily
/// hold more downloads than that cap.
///
/// Each chunk's answers are applied as soon as they arrive rather than
/// buffered for one final apply, so a later chunk failing still leaves the
/// earlier chunks' flags updated. Returns whether every chunk succeeded;
/// [`refresh_stale_flags`] only advances the TTL timestamp on a full `true`,
/// so a partial sweep is retried in full on the next tick rather than being
/// recorded as done.
pub(super) async fn sweep_validators(
    server_url: &str,
    queries: &[omnibus_shared::DownloadValidatorQuery],
) -> bool {
    for chunk in queries.chunks(omnibus_shared::MAX_VALIDATOR_QUERY) {
        let Ok(answers) = crate::data::download_validators(server_url, chunk).await else {
            return false;
        };
        apply_validators(&answers);
    }
    true
}

/// One query per completed download, naming the file it actually came from.
pub(super) fn completed_validator_queries() -> Vec<omnibus_shared::DownloadValidatorQuery> {
    registry()
        .read()
        .map(|m| {
            let mut queries: Vec<_> = m
                .values()
                .filter(|e| matches!(e.status, DownloadStatus::Complete { .. }))
                .map(|e| omnibus_shared::DownloadValidatorQuery {
                    book_uuid: e.book_uuid.clone(),
                    format: match e.format {
                        DlFormat::Epub => omnibus_shared::DownloadFormat::Epub,
                        DlFormat::Audio => omnibus_shared::DownloadFormat::Audio,
                    },
                    file_id: e.file_id,
                })
                .collect();
            // Stable order so a request is reproducible in a log.
            queries.sort_by(|a, b| a.book_uuid.cmp(&b.book_uuid));
            queries
        })
        .unwrap_or_default()
}

/// Apply a batch of server validators to the registry's staleness flags.
///
/// An answer with no etag is "can't tell" — the book, the format or the
/// file is gone, or the scanner has not stat'd it — and leaves the flag as
/// it stands rather than clearing a chip a real comparison set.
pub(super) fn apply_validators(answers: &[omnibus_shared::DownloadValidator]) {
    let mut changed = false;
    for answer in answers {
        let Some(current) = answer.etag.as_deref() else {
            continue;
        };
        let format = match answer.format {
            omnibus_shared::DownloadFormat::Epub => DlFormat::Epub,
            omnibus_shared::DownloadFormat::Audio => DlFormat::Audio,
        };
        let Ok(mut guard) = registry().write() else {
            return;
        };
        let Some(entry) = guard.get_mut(&(answer.book_uuid.clone(), format)) else {
            continue;
        };
        if !matches!(entry.status, DownloadStatus::Complete { .. }) {
            continue;
        }
        let mut snapshots = entry
            .files
            .iter()
            .filter_map(|f| f.source_etag.as_deref())
            .peekable();
        if snapshots.peek().is_none() {
            continue;
        }
        let stale = snapshots.any(|snapshot| snapshot != current);
        if entry.stale != stale {
            entry.stale = stale;
            changed = true;
        }
    }
    if changed {
        bump();
    }
}

/// Whether the stored copy of `(uuid, format)` predates the file currently
/// in the library — the "Update available" condition.
///
/// Deliberately conservative: a missing validator on either side (an old
/// registry row, a `book_files` row the scanner has not stat'd) reports
/// `false`. Not knowing is not the same as knowing it is stale, and
/// prompting a reader to re-download a book on a guess is worse than
/// staying quiet. [`staleness`] keeps the two apart for callers that write
/// the answer rather than render it.
pub fn is_stale(uuid: &str, format: DlFormat, book: &omnibus_shared::EbookMetadata) -> bool {
    staleness(uuid, format, book).unwrap_or(false)
}

/// The comparison behind [`is_stale`], keeping "the file moved" and "this
/// metadata can't tell me" as different answers.
///
/// `None` whenever a validator is missing on either side — an old registry
/// row, a `book_files` row the scanner hasn't stat'd, or the landing
/// projection, which carries no per-file rows at all. Callers that merely
/// render collapse it to `false`; [`note_book`], which *writes* the flag,
/// must not, or a projection-shaped read would clear a chip that a real
/// comparison had set.
fn staleness(uuid: &str, format: DlFormat, book: &omnibus_shared::EbookMetadata) -> Option<bool> {
    let entry = get_entry(uuid, format)?;
    if !matches!(entry.status, DownloadStatus::Complete { .. }) {
        return Some(false);
    }
    let current = target_file(book, format, entry.file_id)?.etag.as_deref()?;
    let mut snapshots = entry
        .files
        .iter()
        .filter_map(|f| f.source_etag.as_deref())
        .peekable();
    snapshots.peek()?;
    Some(snapshots.any(|snapshot| snapshot != current))
}
