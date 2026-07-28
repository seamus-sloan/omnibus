//! Offline download registry: which books are stored locally, per format,
//! with live progress for the UI. The in-memory map is the runtime source
//! of truth (synchronously readable from components and URL builders);
//! every transition is mirrored to the SQLite `downloads` table so state
//! survives a cold start. The fetch loop itself lives in [`engine`].

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

use tokio::sync::watch;

use super::media;
use super::store::{self, DownloadRow};
use super::sync;

mod engine;

/// Downloadable format of a book — one registry row per (uuid, format), so
/// dual-format books hold two independent downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DlFormat {
    Epub,
    Audio,
}

impl DlFormat {
    /// Stable string used in the registry table and status rows.
    pub fn as_str(&self) -> &'static str {
        match self {
            DlFormat::Epub => "epub",
            DlFormat::Audio => "audio",
        }
    }

    fn parse(s: &str) -> Option<DlFormat> {
        match s {
            "epub" => Some(DlFormat::Epub),
            "audio" => Some(DlFormat::Audio),
            _ => None,
        }
    }
}

/// One planned (or fetched) file of a download.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlannedFile {
    /// File name inside `downloads/{uuid}/` (also the `/dl` URL segment).
    pub rel: String,
    /// Server path to fetch, including any `?file_id=` query.
    pub url_path: String,
    /// Audio part ordinal (`None` for the EPUB file).
    pub ordinal: Option<i64>,
    /// Final on-disk size once fetched.
    pub bytes: Option<i64>,
    pub done: bool,
    /// The `ETag` the server served these bytes under, recorded before the
    /// first byte lands so an interrupted transfer can name what its `.part`
    /// file came from. Sent back as `If-Range` on resume: without it the
    /// server has no way to tell that the file changed in between, and the
    /// resumed 206 would splice a new tail onto an old head.
    ///
    /// Distinct from the book's metadata validator — this names the artifact
    /// actually streamed, which for an ebook download is the override-baked
    /// export rather than the source file. `None` for rows written before
    /// downloads recorded it.
    #[serde(default)]
    pub etag: Option<String>,
}

/// UI-facing status of one (uuid, format) download.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    NotDownloaded,
    Downloading { downloaded: i64, total: Option<i64> },
    Error { message: String },
    Complete { bytes: i64 },
}

/// One registry entry (a row that exists in the downloads table).
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadEntry {
    pub book_uuid: String,
    pub format: DlFormat,
    /// Display title snapshot (offline account list).
    pub title: String,
    pub file_id: Option<i64>,
    pub status: DownloadStatus,
    pub files: Vec<PlannedFile>,
    pub updated_at: i64,
    /// `BookFileInfo.validator` as it read when this download was taken.
    ///
    /// Compared against the same field on a network-first metadata refresh to
    /// tell the reader their offline copy has been superseded — a repaired or
    /// re-uploaded file changes the server's side of this while the bytes on
    /// the device stay whatever they were. `None` for downloads taken before
    /// the server reported a validator, which read as current rather than
    /// nagging about a staleness nobody can confirm.
    pub validator: Option<String>,
}

fn registry() -> &'static RwLock<HashMap<(String, DlFormat), DownloadEntry>> {
    static REG: OnceLock<RwLock<HashMap<(String, DlFormat), DownloadEntry>>> = OnceLock::new();
    REG.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Generation counter bumped on every registry change; UI components
/// subscribe via `use_future` and re-read [`status`]/[`list`].
fn channel() -> &'static (watch::Sender<u64>, watch::Receiver<u64>) {
    static CH: OnceLock<(watch::Sender<u64>, watch::Receiver<u64>)> = OnceLock::new();
    CH.get_or_init(|| watch::channel(0))
}

/// Subscribe to registry changes.
pub fn subscribe() -> watch::Receiver<u64> {
    channel().0.subscribe()
}

fn bump() {
    channel().0.send_modify(|g| *g += 1);
}

/// Hydrate the registry from the downloads table. Called from
/// `offline::init` before launch (blocking read — no runtime exists yet).
/// Rows left `downloading` by a previous process die as resumable errors.
pub fn init() {
    let Some(store) = store::store() else { return };
    let rows = store
        .run_blocking(|conn| {
            let mut stmt = match conn.prepare(
                "SELECT book_uuid, format, title, file_id, status, total_bytes,
                        downloaded_bytes, files, error, updated_at, validator
                 FROM downloads",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt.query_map([], |row| {
                Ok(DownloadRow {
                    book_uuid: row.get(0)?,
                    format: row.get(1)?,
                    title: row.get(2)?,
                    file_id: row.get(3)?,
                    status: row.get(4)?,
                    total_bytes: row.get(5)?,
                    downloaded_bytes: row.get(6)?,
                    files: row.get(7)?,
                    error: row.get(8)?,
                    updated_at: row.get(9)?,
                    validator: row.get(10)?,
                })
            });
            match rows {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => Vec::new(),
            }
        })
        .unwrap_or_default();

    let mut map = HashMap::new();
    for row in rows {
        let Some(mut entry) = entry_from_row(row) else {
            continue;
        };
        if matches!(entry.status, DownloadStatus::Downloading { .. }) {
            entry.status = DownloadStatus::Error {
                message: "Interrupted — tap to resume".into(),
            };
            persist(&entry);
        }
        map.insert((entry.book_uuid.clone(), entry.format), entry);
    }
    if let Ok(mut guard) = registry().write() {
        *guard = map;
    }
    bump();
}

/// Status of one (uuid, format).
pub fn status(uuid: &str, format: DlFormat) -> DownloadStatus {
    registry()
        .read()
        .ok()
        .and_then(|m| m.get(&(uuid.to_string(), format)).map(|e| e.status.clone()))
        .unwrap_or(DownloadStatus::NotDownloaded)
}

/// `true` when the format is fully downloaded and locally playable.
pub fn is_complete(uuid: &str, format: DlFormat) -> bool {
    matches!(status(uuid, format), DownloadStatus::Complete { .. })
}

/// `true` when the completed download on this device is no longer the file
/// the server holds — the book was repaired, re-uploaded, or replaced on disk
/// since it was taken offline.
///
/// `book` is whatever the caller most recently read; on a network-first read
/// that is the server's current answer, and offline it is the last one seen,
/// which reports no change rather than a staleness nobody can confirm.
/// Answers `false` for an in-flight or failed download, and for either side
/// missing a validator — the honest reading of "nothing to compare" is "not
/// known to be stale", since the alternative nags at every reader whose
/// library predates the stat backfill.
pub fn is_stale(uuid: &str, format: DlFormat, book: &omnibus_shared::EbookMetadata) -> bool {
    let Some(entry) = get_entry(uuid, format) else {
        return false;
    };
    if !matches!(entry.status, DownloadStatus::Complete { .. }) {
        return false;
    }
    let Some(taken_under) = entry.validator.as_deref() else {
        return false;
    };
    source_validator(book, format, entry.file_id).is_some_and(|now| now != taken_under)
}

/// [`is_stale`] judged against the last book metadata this device saw.
///
/// For callers with no book in hand — the account Downloads list renders
/// straight off the registry. The cache row is written by every book read,
/// so this reflects the most recent online answer and, offline, the last one
/// before that.
pub async fn is_stale_cached(uuid: &str, format: DlFormat) -> bool {
    match super::cache::get_json::<omnibus_shared::EbookMetadata>(&super::cache::keys::ebook(uuid))
        .await
    {
        Some(book) => is_stale(uuid, format, &book),
        None => false,
    }
}

/// The validator of the file a download of this (format, file_id) draws from.
///
/// One definition for both sides of the staleness comparison — the snapshot
/// the engine takes and the refresh [`is_stale`] checks it against. Two
/// copies that disagreed by one tie-break would report every multi-file book
/// as permanently out of date.
///
/// An explicit `file_id` names its own row. Without one, the answer is the
/// server's own — `epub_validator` / `audio_validator` are resolved by the
/// same queries that serve the unqualified download URL, so the two ends
/// cannot disagree about which file it meant, and they are present even for
/// the single-file books whose `book_files` list is never sent.
pub(super) fn source_validator(
    book: &omnibus_shared::EbookMetadata,
    format: DlFormat,
    file_id: Option<i64>,
) -> Option<String> {
    match file_id {
        Some(id) => book
            .book_files
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| f.validator.clone()),
        None => match format {
            DlFormat::Epub => book.epub_validator.clone(),
            DlFormat::Audio => book.audio_validator.clone(),
        },
    }
}

/// Book uuids with at least one complete download (landing-grid badges).
pub fn downloaded_uuids() -> HashSet<String> {
    registry()
        .read()
        .map(|m| {
            m.values()
                .filter(|e| matches!(e.status, DownloadStatus::Complete { .. }))
                .map(|e| e.book_uuid.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Every registry entry, most recently updated first (account screen).
pub fn list() -> Vec<DownloadEntry> {
    let mut entries: Vec<DownloadEntry> = registry()
        .read()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    entries.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
    entries
}

/// Loopback URL for a downloaded EPUB, or `None` when not complete / no
/// loopback server — callers fall back to the tokened server URL.
pub fn local_epub_url(uuid: &str) -> Option<String> {
    if !is_complete(uuid, DlFormat::Epub) {
        return None;
    }
    media::local_file_url(uuid, "book.epub")
}

/// Loopback URL for a downloaded audio part, or `None` when the audio
/// download isn't complete / no loopback server.
pub fn local_part_url(uuid: &str, ordinal: i64) -> Option<String> {
    let rel = registry().read().ok().and_then(|m| {
        let entry = m.get(&(uuid.to_string(), DlFormat::Audio))?;
        if !matches!(entry.status, DownloadStatus::Complete { .. }) {
            return None;
        }
        entry
            .files
            .iter()
            .find(|f| f.ordinal == Some(ordinal))
            .map(|f| f.rel.clone())
    })?;
    media::local_file_url(uuid, &rel)
}

/// Start (or resume) downloading `format` of `uuid`. No-op while already
/// downloading. Requires connectivity — a start with the server unreachable
/// surfaces as an error row, never a queued op (downloads don't queue).
pub fn start(
    server_url: String,
    uuid: String,
    format: DlFormat,
    file_id: Option<i64>,
    title: String,
) {
    if matches!(
        status(&uuid, format),
        DownloadStatus::Downloading { .. } | DownloadStatus::Complete { .. }
    ) {
        return;
    }
    let prior_files = registry()
        .read()
        .ok()
        .and_then(|m| m.get(&(uuid.to_string(), format)).map(|e| e.files.clone()))
        .unwrap_or_default();
    let mut entry = DownloadEntry {
        book_uuid: uuid.clone(),
        format,
        title,
        file_id,
        status: DownloadStatus::Downloading {
            downloaded: 0,
            total: None,
        },
        files: prior_files,
        updated_at: store::now_secs(),
        // Unknown until the engine has the book's metadata in hand; a
        // download in flight is by definition not stale.
        validator: None,
    };
    // Known-offline fast-fail: surface the error row instantly instead of
    // letting the engine burn a doomed connect attempt.
    if sync::is_offline() {
        entry.status = DownloadStatus::Error {
            message: "You're offline — connect to download".into(),
        };
        upsert(entry);
        return;
    }
    upsert(entry);
    let spawned = media::spawn_on_runtime(engine::run(server_url, uuid.clone(), format, file_id));
    if !spawned {
        set_error(&uuid, format, "Offline storage unavailable".into());
    }
}

/// Remove a download: drop the registry row and delete the book's download
/// dir. Ignored while a download is in flight. Per-uuid dirs keep this
/// isolated — no other book's files are touched (AC4).
pub fn remove(uuid: &str, format: DlFormat) {
    if matches!(status(uuid, format), DownloadStatus::Downloading { .. }) {
        return;
    }
    let (uuid, other_format) = (
        uuid.to_string(),
        match format {
            DlFormat::Epub => DlFormat::Audio,
            DlFormat::Audio => DlFormat::Epub,
        },
    );
    let other_exists = registry()
        .read()
        .map(|m| m.contains_key(&(uuid.clone(), other_format)))
        .unwrap_or(false);
    if let Ok(mut guard) = registry().write() {
        guard.remove(&(uuid.clone(), format));
    }
    bump();
    let spawned = media::spawn_on_runtime(async move {
        if let Some(store) = store::store() {
            store.downloads_delete(&uuid, format.as_str()).await;
        }
        let Some(dir) = media::downloads_root().map(|r| r.join(&uuid)) else {
            return;
        };
        if other_exists {
            // The sibling format is still downloaded: remove only this
            // format's files, leaving the shared dir in place.
            remove_format_files(&dir, format).await;
        } else {
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
    });
    if !spawned {
        tracing::warn!("offline runtime unavailable; download files not removed");
    }
}

/// Delete just one format's files from a shared per-book dir.
async fn remove_format_files(dir: &std::path::Path, format: DlFormat) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_epub_file = name.starts_with("book.epub");
        let is_part_file = name.starts_with("part-");
        let matches_format = match format {
            DlFormat::Epub => is_epub_file,
            DlFormat::Audio => is_part_file,
        };
        if matches_format {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Total bytes across complete downloads (account storage line).
pub fn total_complete_bytes() -> i64 {
    registry()
        .read()
        .map(|m| {
            m.values()
                .filter_map(|e| match e.status {
                    DownloadStatus::Complete { bytes } => Some(bytes),
                    _ => None,
                })
                .sum()
        })
        .unwrap_or(0)
}

// --- registry/persistence plumbing shared with the engine ---

pub(super) fn upsert(entry: DownloadEntry) {
    persist(&entry);
    if let Ok(mut guard) = registry().write() {
        guard.insert((entry.book_uuid.clone(), entry.format), entry);
    }
    bump();
}

pub(super) fn get_entry(uuid: &str, format: DlFormat) -> Option<DownloadEntry> {
    registry()
        .read()
        .ok()
        .and_then(|m| m.get(&(uuid.to_string(), format)).cloned())
}

/// Mid-stream progress bump: mutates the byte-count fields on the existing
/// registry entry in place (no full-entry/`Vec<PlannedFile>` clone) and
/// mirrors just those columns to SQLite via a targeted `UPDATE`, skipping
/// the JSON `files` re-serialize a full [`upsert`] pays. No-op if the entry
/// isn't registered yet (a progress tick racing a `remove`).
pub(super) fn update_progress_bytes(
    uuid: &str,
    format: DlFormat,
    downloaded: i64,
    total: Option<i64>,
) {
    let downloaded = downloaded.max(0);
    let now = store::now_secs();
    let Ok(mut guard) = registry().write() else {
        return;
    };
    let Some(entry) = guard.get_mut(&(uuid.to_string(), format)) else {
        return;
    };
    entry.status = DownloadStatus::Downloading { downloaded, total };
    entry.updated_at = now;
    // Release the lock before the DB write — persistence never needs to
    // hold it.
    drop(guard);
    persist_progress(uuid, format, downloaded, total, now);
    bump();
}

fn persist_progress(
    uuid: &str,
    format: DlFormat,
    downloaded: i64,
    total: Option<i64>,
    updated_at: i64,
) {
    let Some(store) = store::store() else { return };
    let uuid = uuid.to_string();
    let format = format.as_str();
    store.run_detached(move |conn| {
        store::log_err(conn.execute(
            "UPDATE downloads SET downloaded_bytes = ?1, total_bytes = ?2, updated_at = ?3
             WHERE book_uuid = ?4 AND format = ?5",
            rusqlite::params![downloaded, total, updated_at, uuid, format],
        ));
    });
}

/// Record the validator one file's bytes are being served under.
///
/// Written the moment the response headers land, before any of the body is on
/// disk, because the case it exists for is the process dying mid-transfer: on
/// the next attempt the `.part` file is all that survived, and this is what
/// says which server-side bytes it was cut from. No-op when the entry or file
/// isn't registered (a header callback racing a `remove`).
pub(super) fn set_file_etag(uuid: &str, format: DlFormat, rel: &str, etag: Option<String>) {
    let entry = {
        let Ok(mut guard) = registry().write() else {
            return;
        };
        let Some(entry) = guard.get_mut(&(uuid.to_string(), format)) else {
            return;
        };
        let Some(file) = entry.files.iter_mut().find(|f| f.rel == rel) else {
            return;
        };
        if file.etag == etag {
            return;
        }
        file.etag = etag;
        entry.updated_at = store::now_secs();
        entry.clone()
    };
    // Full `persist` rather than a targeted UPDATE: the validator lives in the
    // JSON `files` blob, so there is no column to write on its own.
    persist(&entry);
}

pub(super) fn set_error(uuid: &str, format: DlFormat, message: String) {
    let Some(mut entry) = get_entry(uuid, format) else {
        return;
    };
    entry.status = DownloadStatus::Error { message };
    entry.updated_at = store::now_secs();
    upsert(entry);
}

fn persist(entry: &DownloadEntry) {
    let Some(store) = store::store() else { return };
    let row = row_from_entry(entry);
    let store = store.clone();
    // Fire-and-forget mirror; the in-memory registry stays authoritative.
    store.run_detached(move |conn| {
        let _ = conn.execute(
            "INSERT INTO downloads
               (book_uuid, format, title, file_id, status, total_bytes, downloaded_bytes, files, error, updated_at, validator)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(book_uuid, format) DO UPDATE SET
               title = ?3, file_id = ?4, status = ?5, total_bytes = ?6,
               downloaded_bytes = ?7, files = ?8, error = ?9, updated_at = ?10,
               validator = ?11",
            rusqlite::params![
                row.book_uuid,
                row.format,
                row.title,
                row.file_id,
                row.status,
                row.total_bytes,
                row.downloaded_bytes,
                row.files,
                row.error,
                row.updated_at,
                row.validator,
            ],
        );
    });
}

fn row_from_entry(entry: &DownloadEntry) -> DownloadRow {
    let (status, total, downloaded, error) = match &entry.status {
        DownloadStatus::NotDownloaded => ("error", None, 0, Some("removed".to_string())),
        DownloadStatus::Downloading { downloaded, total } => {
            ("downloading", *total, *downloaded, None)
        }
        DownloadStatus::Error { message } => ("error", None, 0, Some(message.clone())),
        DownloadStatus::Complete { bytes } => ("complete", Some(*bytes), *bytes, None),
    };
    DownloadRow {
        book_uuid: entry.book_uuid.clone(),
        format: entry.format.as_str().to_string(),
        title: entry.title.clone(),
        file_id: entry.file_id,
        status: status.to_string(),
        total_bytes: total,
        downloaded_bytes: downloaded,
        files: serde_json::to_string(&entry.files).unwrap_or_else(|_| "[]".into()),
        error,
        updated_at: entry.updated_at,
        validator: entry.validator.clone(),
    }
}

fn entry_from_row(row: DownloadRow) -> Option<DownloadEntry> {
    let format = DlFormat::parse(&row.format)?;
    let files: Vec<PlannedFile> = serde_json::from_str(&row.files).unwrap_or_default();
    let status = match row.status.as_str() {
        "downloading" => DownloadStatus::Downloading {
            downloaded: row.downloaded_bytes,
            total: row.total_bytes,
        },
        "complete" => DownloadStatus::Complete {
            bytes: row.downloaded_bytes,
        },
        _ => DownloadStatus::Error {
            message: row.error.unwrap_or_else(|| "Download failed".into()),
        },
    };
    Some(DownloadEntry {
        book_uuid: row.book_uuid,
        format,
        title: row.title,
        file_id: row.file_id,
        status,
        files,
        updated_at: row.updated_at,
        validator: row.validator,
    })
}

/// Default audio file for playback/download: lowest-ordinal M4B/M4A/MP3.
/// Mirrors the HLS resolver's accepted formats; shared with the mobile
/// listen host so download and playback pick the same file.
pub fn default_audio_file_id(files: &[omnibus_shared::BookFileInfo]) -> Option<i64> {
    files
        .iter()
        .filter(|f| {
            f.format.eq_ignore_ascii_case("M4B")
                || f.format.eq_ignore_ascii_case("M4A")
                || f.format.eq_ignore_ascii_case("MP3")
        })
        .min_by_key(|f| f.ordinal)
        .map(|f| f.id)
}

/// Human-readable size for the download UI (binary units, one decimal).
pub fn format_bytes(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value as i64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests;
