//! The download fetch loop: plan the files for a (book, format), stream
//! them to `.part` temp files with Range-based resume, rename on
//! completion, and warm the metadata/manifest/artwork caches so the book
//! is fully usable offline. Runs on the loopback media runtime so
//! downloads survive route changes and component unmounts.

use omnibus_shared::{AudiobookManifest, EbookMetadata, ManifestPart};

use anyhow::Context;

use crate::data;
use crate::offline::{cache, media};

use super::{DlFormat, DownloadEntry, DownloadStatus, PlannedFile};

/// Flush registry progress every this many streamed bytes.
const PROGRESS_FLUSH_BYTES: i64 = 1024 * 1024;

/// Fixed, user-safe download failure reasons. Every other fallible step in
/// this module (filesystem, network I/O) is wrapped with a safe
/// `.context(...)` message instead, so no raw `io`/`reqwest` `Display` ever
/// reaches [`DownloadStatus::Error`] — see
/// `.claude/rules/02-error-handling.md`'s boundary rule.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
enum DownloadError {
    #[error("You're offline — connect to download")]
    Offline,
    #[error("Book not found")]
    NotFound,
    #[error("This audiobook's format isn't supported offline")]
    UnsupportedFormat,
    #[error("Nothing to download for this format")]
    NothingToDownload,
    #[error("Offline storage unavailable")]
    StorageUnavailable,
    #[error("Connection lost")]
    ConnectionLost,
    #[error("Server returned an unexpected response")]
    ServerError,
    #[error("Network error — check your connection and try again")]
    NetworkError,
    #[error("You need to sign in again")]
    Unauthorized,
    #[error("Download was interrupted — tap to resume")]
    Interrupted,
}

/// Subdirectory a replacement downloads into before it is swapped over the
/// copy it supersedes. Lives under the book's own dir so it is cleaned up with
/// it, and is never served: the loopback `/dl/{uuid}/{file}` route only ever
/// resolves plain file names.
const REPLACE_DIR: &str = ".replacing";

/// Run one download to completion (or error). The registry entry was
/// already put in `Downloading` by `downloads::start`.
///
/// `replacing` carries the completed entry a re-download supersedes, if any.
/// Its presence stages the transfer and, on failure, restores it.
pub(super) async fn run(
    server_url: String,
    uuid: String,
    format: DlFormat,
    file_id: Option<i64>,
    replacing: Option<DownloadEntry>,
) {
    if let Err(err) = run_inner(&server_url, &uuid, format, file_id, replacing.is_some()).await {
        // `err`'s `Display` is always a safe, pre-authored message (see
        // `DownloadError` and the `.context(...)` calls below); the full
        // chain — which can include raw io/reqwest text — is logged here
        // for diagnosis and never forwarded to the UI.
        tracing::warn!(error = ?err, book_uuid = %uuid, "offline download failed");
        match replacing {
            // A failed replacement never touched the copy on disk, so the
            // honest end state is the one the reader still has. An error row
            // would hide a book they can still open — and the staleness chip
            // stays put, which is both the truth and the retry affordance.
            Some(prior) => {
                if let Some(dir) = media::downloads_root().map(|r| r.join(&uuid)) {
                    let _ = tokio::fs::remove_dir_all(dir.join(REPLACE_DIR)).await;
                }
                super::upsert(prior);
            }
            None => super::set_error(&uuid, format, err.to_string()),
        }
    }
}

async fn run_inner(
    server_url: &str,
    uuid: &str,
    format: DlFormat,
    file_id: Option<i64>,
    replacing: bool,
) -> anyhow::Result<()> {
    // The raw `_online` variant: starting a download while offline must
    // fail loudly, never silently plan from a stale cached book.
    let book = data::get_ebook_online(server_url, uuid)
        .await
        .map_err(|e| friendly(&e))?
        .ok_or(DownloadError::NotFound)?;
    cache::put_json(&cache::keys::ebook(uuid), &book);
    let title = book
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| book.filename.clone());

    let (plan, total_estimate) = match format {
        DlFormat::Epub => (plan_epub(uuid, file_id), epub_size_estimate(&book)),
        DlFormat::Audio => {
            let fid = file_id.or_else(|| super::default_audio_file_id(&book.book_files));
            let manifest = data::get_manifest_online(server_url, uuid, fid)
                .await
                .map_err(|e| friendly(&e))?;
            let parts = match manifest {
                AudiobookManifest::Direct { ref parts, .. } => parts.clone(),
                AudiobookManifest::Hls { .. } => {
                    // The mobile player can't play HLS-only books either, so
                    // there is nothing useful to store.
                    return Err(DownloadError::UnsupportedFormat.into());
                }
            };
            // The offline player reads the manifest from this cache row.
            cache::put_json(&cache::keys::manifest(uuid, fid), &manifest);
            (plan_audio_parts(&parts), audio_size_estimate(&book))
        }
    };
    if plan.is_empty() {
        return Err(DownloadError::NothingToDownload.into());
    }

    let live_dir = media::downloads_root()
        .map(|r| r.join(uuid))
        .ok_or(DownloadError::StorageUnavailable)?;
    // A replacement is staged beside the copy it supersedes, never written
    // over it in place: the reader keeps a book they can open until the new
    // one is complete, and a transfer that fails — including the offline
    // fast-fail — costs them nothing.
    let dir = if replacing {
        let staging = live_dir.join(REPLACE_DIR);
        // Anything left by an abandoned attempt is of unknown provenance.
        let _ = tokio::fs::remove_dir_all(&staging).await;
        staging
    } else {
        live_dir.clone()
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .context("Offline storage unavailable")?;

    let mut validator = super::source_validator(&book, format, file_id);
    let previous_entry = super::get_entry(uuid, format);

    // Prior work is only reusable if it came from the source this attempt is
    // downloading. A multi-part audiobook interrupted before a replacement
    // would otherwise keep its finished tracks from the old file, fetch the
    // rest from the new one, and report the mixture as current — the splice
    // `If-Range` prevents inside one file, happening across several.
    //
    // Unprovable counts as unusable: an entry from before validators were
    // recorded costs one restart, which is cheaper than a silently mixed book.
    let same_source = matches!(
        (&previous_entry.as_ref().and_then(|e| e.validator.as_deref()), &validator.as_deref()),
        (Some(before), Some(now)) if before == now
    );
    let prior: Vec<PlannedFile> = match previous_entry {
        Some(entry) if same_source => entry.files,
        _ => Vec::new(),
    };

    // Merge prior completion state (resume of a partly-finished download).
    // Stat asynchronously — this future shares the loopback runtime with the
    // media server, so blocking std::fs calls here would stall playback.
    let mut files: Vec<PlannedFile> = Vec::with_capacity(plan.len());
    for mut f in plan {
        let previous = prior.iter().find(|p| p.rel == f.rel);
        // Carry the recorded validator across the replan, for finished files
        // and half-finished ones alike: a fresh plan knows the URL but not
        // what the bytes already on disk were served under, and dropping it
        // would leave the resume with no precondition to send. Discarding the
        // prior plan above therefore also disarms every resume below, since
        // `download_file` refuses to resume a partial it cannot vouch for.
        f.etag = previous.and_then(|p| p.etag.clone());
        if previous.is_some_and(|p| p.done) {
            if let Ok(meta) = tokio::fs::metadata(dir.join(&f.rel)).await {
                if meta.is_file() {
                    f.done = true;
                    f.bytes = Some(meta.len() as i64);
                }
            }
        }
        files.push(f);
    }
    let mut downloaded: i64 = files.iter().filter_map(|f| f.bytes).sum();
    publish(
        uuid,
        format,
        &title,
        file_id,
        &files,
        downloaded,
        total_estimate,
        validator.as_deref(),
    );

    for idx in 0..files.len() {
        if files[idx].done {
            continue;
        }
        let mut unflushed: i64 = 0;
        let planned = files[idx].clone();
        let fetched = download_file(server_url, uuid, format, &dir, &planned, &mut |delta| {
            downloaded += delta;
            unflushed += delta;
            if unflushed >= PROGRESS_FLUSH_BYTES {
                unflushed = 0;
                // Re-publish with the pre-loop file list; per-file `done`
                // flags land on file completion below.
                publish_progress(uuid, format, downloaded, total_estimate);
            }
        })
        .await?;
        files[idx].done = true;
        files[idx].bytes = Some(fetched.bytes);
        files[idx].etag = fetched.etag;
        // The server's own answer about what it served, which beats the
        // metadata read from before the transfer: a source replaced in between
        // would otherwise have this copy recorded under a validator that never
        // described it.
        if fetched.source.is_some() {
            validator = fetched.source;
        }
        downloaded = files.iter().filter_map(|f| f.bytes).sum();
        publish(
            uuid,
            format,
            &title,
            file_id,
            &files,
            downloaded,
            total_estimate,
            validator.as_deref(),
        );
    }

    if replacing {
        swap_into_place(&dir, &live_dir, &files).await?;
    }

    warm_related(server_url, uuid, &book, format).await;

    let bytes: i64 = files.iter().filter_map(|f| f.bytes).sum();
    super::upsert(DownloadEntry {
        book_uuid: uuid.to_string(),
        format,
        title,
        file_id,
        status: DownloadStatus::Complete { bytes },
        files,
        updated_at: crate::offline::store::now_secs(),
        validator,
    });
    Ok(())
}

/// The server header naming the `book_files` row a download drew from. Kept in
/// lockstep with `backend::validator::SOURCE_VALIDATOR`.
const SOURCE_VALIDATOR_HEADER: &str = "x-omnibus-source-validator";

/// What one completed transfer knows about itself.
struct Fetched {
    bytes: i64,
    /// `ETag` of the artifact streamed — the `If-Range` precondition for a
    /// later resume of this same file.
    etag: Option<String>,
    /// Validator of the source row the server resolved, which is what a later
    /// metadata refresh is compared against. Distinct from `etag` wherever the
    /// bytes served are not the source file itself: one part of a multi-part
    /// audiobook, or an override-baked export.
    source: Option<String>,
}

/// Move a finished replacement over the copy it supersedes, then drop the
/// staging dir.
///
/// Runs only once every file has landed, so the swap is the first moment the
/// reader's copy changes at all. Each rename is atomic within the book's own
/// directory; a failure part-way leaves a mixed set that the next attempt
/// re-downloads whole, since the entry is restored to its pre-replacement
/// validator and no longer matches the source.
async fn swap_into_place(
    staged: &std::path::Path,
    live: &std::path::Path,
    files: &[PlannedFile],
) -> anyhow::Result<()> {
    for file in files {
        tokio::fs::rename(staged.join(&file.rel), live.join(&file.rel))
            .await
            .context("Could not replace the copy on this device")?;
    }
    let _ = tokio::fs::remove_dir_all(staged).await;
    Ok(())
}

/// Stream one file to `{rel}.part`, resuming from any existing bytes via a
/// Range request, then rename to `rel`. Returns the final byte count.
///
/// Returns the final byte count and the validator the server served those
/// bytes under, which is also written to the registry — awaited, before a
/// single byte is on disk — so a transfer that dies mid-stream still leaves a
/// `.part` the next attempt can vouch for.
async fn download_file(
    server_url: &str,
    uuid: &str,
    format: DlFormat,
    dir: &std::path::Path,
    file: &PlannedFile,
    on_delta: &mut (dyn FnMut(i64) + Send),
) -> anyhow::Result<Fetched> {
    use tokio::io::AsyncWriteExt;

    let part_path = dir.join(format!("{}.part", file.rel));
    let final_path = dir.join(&file.rel);
    let resumed = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let url = format!("{server_url}{}", file.url_path);
    // Streaming client: no whole-request timeout, so a large book can't be
    // killed mid-transfer by the default client's 30s cap.
    let mut req = data::with_bearer(data::streaming_client().get(&url));
    // A `Range` only ever goes out under a precondition. Without one the
    // server cannot tell the file was replaced since the interrupted attempt,
    // and the 206 it returns would be counted from the *new* bytes — appending
    // a new tail to the old head already in `.part` and leaving a corrupt book
    // that nothing reports. So a partial whose validator was never recorded —
    // a `.part` from before this was tracked, or one whose process died before
    // the write landed — is not resumed at all: asking for the whole file
    // routes it through the restart branch below, which truncates it.
    if let (true, Some(etag)) = (resumed > 0, file.etag.as_deref()) {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(etag) {
            req = req
                .header(reqwest::header::RANGE, format!("bytes={resumed}-"))
                .header(reqwest::header::IF_RANGE, value);
        }
    }
    let mut resp = req.send().await.map_err(|e| {
        tracing::warn!(error = %e, url = %file.url_path, "offline download: request failed");
        DownloadError::ConnectionLost
    })?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        tracing::warn!(status = status.as_u16(), url = %file.url_path, "offline download: request was unauthorized");
        return Err(DownloadError::Unauthorized.into());
    }
    if !status.is_success() {
        tracing::warn!(status = status.as_u16(), url = %file.url_path, "offline download: server returned a non-success status");
        return Err(DownloadError::ServerError.into());
    }

    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let etag = header(reqwest::header::ETAG.as_str());
    let source = header(SOURCE_VALIDATOR_HEADER);
    super::set_file_etag(uuid, format, &file.rel, etag.clone()).await;

    // 206 → append after the existing bytes; 200 → the server ignored the
    // Range (or refused it because the file changed), start over.
    let appending = status == reqwest::StatusCode::PARTIAL_CONTENT && resumed > 0;
    let expected_total = resp
        .content_length()
        .map(|cl| if appending { resumed + cl } else { cl });
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .append(appending)
        .write(true)
        .truncate(!appending)
        .open(&part_path)
        .await
        .context("Could not save the download to this device")?;
    if appending {
        on_delta(0);
    } else if resumed > 0 {
        // Restarted from scratch: the previously-counted bytes are gone.
        on_delta(-(resumed as i64));
    }

    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, url = %file.url_path, "offline download: chunk read failed");
                return Err(DownloadError::ConnectionLost.into());
            }
        };
        out.write_all(&chunk)
            .await
            .context("Could not save the download to this device")?;
        on_delta(chunk.len() as i64);
    }
    out.flush()
        .await
        .context("Could not save the download to this device")?;
    out.sync_all()
        .await
        .context("Could not save the download to this device")?;
    drop(out);

    let written = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len() as i64)
        .context("Could not verify the downloaded file")?;
    if let Some(expected) = expected_total {
        if written != expected as i64 {
            return Err(DownloadError::Interrupted.into());
        }
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .context("Could not finalize the download")?;
    Ok(Fetched {
        bytes: written,
        etag,
        source,
    })
}

/// Warm every cache a downloaded book needs offline: artwork (lock screen,
/// grid, detail hero) plus the reader/player sidecar data. The data calls
/// go through the caching wrappers, whose success path writes the cache
/// rows itself. Best-effort — the book files are already safe on disk.
async fn warm_related(server_url: &str, uuid: &str, book: &EbookMetadata, format: DlFormat) {
    for size in ["sm", "md", "lg"] {
        media::warm_image(server_url, &format!("/api/thumbs/{uuid}/{size}")).await;
    }
    if let Some(cover) = &book.cover_url {
        media::warm_image(server_url, cover).await;
    }
    let progress_format = match format {
        DlFormat::Epub => omnibus_shared::ProgressFormat::Epub,
        DlFormat::Audio => omnibus_shared::ProgressFormat::Audio,
    };
    let _ = data::get_progress(server_url, uuid, progress_format).await;
    let _ = data::list_highlights(server_url, uuid).await;
    let _ = data::list_bookmarks(server_url, uuid).await;
    if format == DlFormat::Audio {
        let _ = data::get_playback_rate(server_url, uuid).await;
    }
}

pub(super) fn plan_epub(uuid: &str, file_id: Option<i64>) -> Vec<PlannedFile> {
    let query = file_id
        .map(|id| format!("?file_id={id}"))
        .unwrap_or_default();
    vec![PlannedFile {
        rel: "book.epub".into(),
        url_path: format!("/api/ebooks/{uuid}/download{query}"),
        ordinal: None,
        bytes: None,
        done: false,
        // Filled in from the prior entry by the replan merge in `run_inner`,
        // then from the response headers once the transfer starts.
        etag: None,
    }]
}

/// One planned file per direct-play part; the `rel` extension round-trips
/// through `media::ext_mime` so the loopback server serves the right type.
pub(super) fn plan_audio_parts(parts: &[ManifestPart]) -> Vec<PlannedFile> {
    parts
        .iter()
        .map(|p| PlannedFile {
            rel: format!("part-{}.{}", p.ordinal, mime_ext(&p.mime)),
            url_path: p.url.clone(),
            ordinal: Some(p.ordinal),
            bytes: None,
            done: false,
            etag: None,
        })
        .collect()
}

/// Extension for a manifest part's mime type. Inverse of
/// `media::ext_mime` — keep the two maps in sync.
pub(super) fn mime_ext(mime: &str) -> &'static str {
    match mime {
        "audio/mp4" | "audio/x-m4b" | "audio/m4b" => "m4b",
        "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/aac" => "aac",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" => "wav",
        _ => "m4b",
    }
}

pub(super) fn epub_size_estimate(book: &EbookMetadata) -> Option<i64> {
    book.book_files
        .iter()
        .find(|f| f.format.eq_ignore_ascii_case("EPUB") && f.size_bytes > 0)
        .map(|f| f.size_bytes)
}

pub(super) fn audio_size_estimate(book: &EbookMetadata) -> Option<i64> {
    let sum: i64 = book
        .book_files
        .iter()
        .filter(|f| {
            f.format.eq_ignore_ascii_case("M4B")
                || f.format.eq_ignore_ascii_case("M4A")
                || f.format.eq_ignore_ascii_case("MP3")
        })
        .map(|f| f.size_bytes)
        .sum();
    (sum > 0).then_some(sum)
}

#[allow(clippy::too_many_arguments)]
fn publish(
    uuid: &str,
    format: DlFormat,
    title: &str,
    file_id: Option<i64>,
    files: &[PlannedFile],
    downloaded: i64,
    total: Option<i64>,
    validator: Option<&str>,
) {
    super::upsert(DownloadEntry {
        book_uuid: uuid.to_string(),
        format,
        title: title.to_string(),
        file_id,
        status: DownloadStatus::Downloading {
            downloaded: downloaded.max(0),
            total,
        },
        files: files.to_vec(),
        updated_at: crate::offline::store::now_secs(),
        validator: validator.map(str::to_string),
    });
}

/// Cheap mid-stream progress bump that keeps the existing file list — see
/// [`super::update_progress_bytes`] for why this avoids the full-entry
/// clone + JSON re-serialize `publish`/`upsert` pay on a status change.
fn publish_progress(uuid: &str, format: DlFormat, downloaded: i64, total: Option<i64>) {
    super::update_progress_bytes(uuid, format, downloaded, total);
}

/// Maps a [`crate::data::DataError`] to a fixed, user-safe [`DownloadError`].
/// The original error's `Display` (which can carry raw `reqwest`/
/// `serde_json` transport/decode text) is logged via `tracing::warn!` for
/// diagnosis but never returned — only the fixed variant reaches the UI.
fn friendly(e: &crate::data::DataError) -> DownloadError {
    tracing::warn!(error = %e, "offline download: data fetch failed");
    if crate::offline::sync::is_offline_error(e) {
        return DownloadError::Offline;
    }
    use crate::data::DataError;
    match e {
        DataError::Offline => DownloadError::Offline,
        DataError::Unauthorized => DownloadError::Unauthorized,
        // A `Network` variant that *is* a decode failure means the server
        // was reachable but returned a malformed body — same bucket as an
        // explicit `Decode`/`Http`, not a connectivity problem.
        DataError::Network(re) if re.is_decode() => DownloadError::ServerError,
        DataError::Network(_) => DownloadError::NetworkError,
        DataError::Http { .. } | DataError::Decode(_) => DownloadError::ServerError,
        DataError::Other(_) => DownloadError::NetworkError,
    }
}

#[cfg(test)]
mod tests;
