//! The download fetch loop: plan the files for a (book, format), stream
//! them to `.part` temp files with Range-based resume, rename on
//! completion, and warm the metadata/manifest/artwork caches so the book
//! is fully usable offline. Runs on the loopback media runtime so
//! downloads survive route changes and component unmounts.

use anyhow::Context;
use omnibus_shared::{AudiobookManifest, EbookMetadata, ManifestPart};

use crate::data;
use crate::offline::{cache, media};

use super::verify::{self, Verdict};
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
    #[error("The download arrived damaged — tap to try again")]
    Damaged,
}

/// Run one download to completion (or error). The registry entry was
/// already put in `Downloading` by `downloads::start`.
pub(super) async fn run(server_url: String, uuid: String, format: DlFormat, file_id: Option<i64>) {
    if let Err(err) = run_inner(&server_url, &uuid, format, file_id).await {
        // `err`'s `Display` is always a safe, pre-authored message (see
        // `DownloadError` and the `.context(...)` calls below); the full
        // chain — which can include raw io/reqwest text — is logged here
        // for diagnosis and never forwarded to the UI.
        tracing::warn!(error = ?err, book_uuid = %uuid, "offline download failed");
        super::set_error(&uuid, format, err.to_string());
    }
}

/// Run a replacement download, restoring `previous` if it fails.
///
/// The files `previous` describes are still on disk throughout: the engine
/// writes `.part` siblings and renames over them only on success. So a
/// failure here is not a loss — it is the book the reader already had,
/// still complete and still flagged, with the chip inviting another try.
pub(super) async fn run_replacing(
    server_url: String,
    uuid: String,
    format: DlFormat,
    file_id: Option<i64>,
    previous: DownloadEntry,
) {
    if let Err(err) = run_inner(&server_url, &uuid, format, file_id).await {
        tracing::warn!(
            error = ?err,
            book_uuid = %uuid,
            "offline re-download failed; keeping the copy already on the device"
        );
        super::restore(previous);
    }
}

async fn run_inner(
    server_url: &str,
    uuid: &str,
    format: DlFormat,
    file_id: Option<i64>,
) -> anyhow::Result<()> {
    let (book, title) = load_book(server_url, uuid).await?;

    // Snapshot the wire validator of the `book_files` row this download
    // targets. A later metadata refresh compares against it to learn the
    // library file was replaced — see `downloads::is_stale`.
    let source_etag = super::target_file(&book, format, file_id).and_then(|f| f.etag.clone());

    let (plan, total_estimate) = plan_for_format(server_url, uuid, file_id, &book, format).await?;
    if plan.is_empty() {
        return Err(DownloadError::NothingToDownload.into());
    }

    let dir = media::downloads_root()
        .map(|r| r.join(uuid))
        .ok_or(DownloadError::StorageUnavailable)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .context("Offline storage unavailable")?;

    let mut files = prepare_files(&dir, uuid, format, source_etag, plan).await;

    let downloaded: i64 = files.iter().filter_map(|f| f.bytes).sum();
    publish(
        uuid,
        format,
        &title,
        file_id,
        &files,
        downloaded,
        total_estimate,
    );

    run_downloads(
        server_url,
        uuid,
        format,
        &dir,
        file_id,
        &title,
        &mut files,
        downloaded,
        total_estimate,
    )
    .await?;

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
        stale: false,
    });
    Ok(())
}

/// Fetch the book's live metadata, cache it, and derive its display title.
///
/// Uses the raw `_online` variant: starting a download while offline must
/// fail loudly, never silently plan from a stale cached book.
async fn load_book(server_url: &str, uuid: &str) -> anyhow::Result<(EbookMetadata, String)> {
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
    Ok((book, title))
}

/// Build the file plan and size estimate for `format`, fetching (and
/// caching) the audiobook manifest first when needed.
async fn plan_for_format(
    server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
    book: &EbookMetadata,
    format: DlFormat,
) -> anyhow::Result<(Vec<PlannedFile>, Option<i64>)> {
    Ok(match format {
        DlFormat::Epub => (plan_epub(uuid, file_id), epub_size_estimate(book)),
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
            (plan_audio_parts(&parts), audio_size_estimate(book))
        }
    })
}

/// Merge prior completion state (resume of a partly-finished download) onto
/// a fresh plan. Stat asynchronously — this future shares the loopback
/// runtime with the media server, so blocking std::fs calls here would
/// stall playback.
async fn prepare_files(
    dir: &std::path::Path,
    uuid: &str,
    format: DlFormat,
    source_etag: Option<String>,
    plan: Vec<PlannedFile>,
) -> Vec<PlannedFile> {
    let prior = super::get_entry(uuid, format)
        .map(|e| e.files)
        .unwrap_or_default();
    let mut files: Vec<PlannedFile> = Vec::with_capacity(plan.len());
    for mut f in plan {
        f.source_etag = source_etag.clone();
        if let Some(previous) = prior.iter().find(|p| p.rel == f.rel) {
            if !reusable(previous.source_etag.as_deref(), source_etag.as_deref()) {
                // These bytes cannot be shown to belong to the file this
                // attempt is planning against. Keeping them would assemble
                // one audiobook out of two — part 0 from the old recording,
                // the rest from the new — and stamp every part with the
                // current validator, so nothing downstream would ever
                // report it. Start this file over instead.
                discard_partial(dir, &f.rel).await;
            } else {
                // Carry the prior attempt's response validator forward so
                // the resume below can offer it as `If-Range`.
                f.http_etag = previous.http_etag.clone();
                if previous.done {
                    if let Ok(meta) = tokio::fs::metadata(dir.join(&f.rel)).await {
                        if meta.is_file() {
                            f.done = true;
                            f.bytes = Some(meta.len() as i64);
                        }
                    }
                }
            }
        }
        files.push(f);
    }
    files
}

/// Download every not-yet-`done` planned file in order, publishing progress
/// (and per-file completion) along the way. Returns the total bytes
/// downloaded across all files once the loop completes.
#[allow(clippy::too_many_arguments)]
async fn run_downloads(
    server_url: &str,
    uuid: &str,
    format: DlFormat,
    dir: &std::path::Path,
    file_id: Option<i64>,
    title: &str,
    files: &mut [PlannedFile],
    mut downloaded: i64,
    total_estimate: Option<i64>,
) -> anyhow::Result<i64> {
    for idx in 0..files.len() {
        if files[idx].done {
            continue;
        }
        let mut unflushed: i64 = 0;
        let mut observed_etag: Option<String> = None;
        // Cloned so the header callback can write to the registry while the
        // planned file is still borrowed by the call.
        let planned = files[idx].clone();
        let result = download_file(
            server_url,
            dir,
            &planned,
            &mut |etag: Option<String>| {
                observed_etag.clone_from(&etag);
                // Straight to disk, before a body byte is read. Waiting for
                // the transfer to return — even on its error path — misses
                // the interruptions that actually happen on a phone: the
                // process being killed, or the runtime dropping the task.
                // Both leave the `.part` behind, and a `.part` without its
                // tag resumes with a bare `Range`.
                super::record_file_etag(uuid, format, &planned.rel, etag);
            },
            &mut |delta| {
                downloaded += delta;
                unflushed += delta;
                if unflushed >= PROGRESS_FLUSH_BYTES {
                    unflushed = 0;
                    // Re-publish with the pre-loop file list; per-file `done`
                    // flags land on file completion below.
                    publish_progress(uuid, format, downloaded, total_estimate);
                }
            },
        )
        .await;
        files[idx].http_etag = observed_etag;
        let fetched = result?;
        files[idx].done = true;
        files[idx].bytes = Some(fetched.bytes);
        downloaded = files.iter().filter_map(|f| f.bytes).sum();
        publish(
            uuid,
            format,
            title,
            file_id,
            files,
            downloaded,
            total_estimate,
        );
    }
    Ok(downloaded)
}

/// Whether bytes already on disk can be shown to have come from the file
/// the current plan names.
#[derive(Debug, PartialEq, Eq)]
enum Provenance {
    /// Same file, proven — reuse the bytes.
    Proven,
    /// A different file, proven — the bytes are from another edition.
    Superseded,
    /// Unprovable: one side or the other published no validator.
    Unknown,
}

fn provenance(previous: Option<&str>, current: Option<&str>) -> Provenance {
    match (previous, current) {
        (Some(a), Some(b)) if a == b => Provenance::Proven,
        (Some(_), Some(_)) => Provenance::Superseded,
        _ => Provenance::Unknown,
    }
}

/// Whether a part already on disk may be carried into this attempt.
///
/// `Unknown` is reusable only while the *current* file has no validator
/// either — nothing has been learned, so refusing to resume would just
/// break downloads against a row the scanner has never stat'd. Once the
/// current file does have one, an unprovable part is not kept: it would be
/// silently restamped with that validator, which turns "I have no idea
/// where these bytes came from" into "these bytes are current" and lets a
/// pre-validator part sit in the same audiobook as freshly-fetched ones
/// with nothing downstream ever reporting it. Re-fetching costs a
/// download; the alternative costs a book that is quietly wrong.
fn reusable(previous: Option<&str>, current: Option<&str>) -> bool {
    match provenance(previous, current) {
        Provenance::Proven => true,
        Provenance::Superseded => false,
        Provenance::Unknown => current.is_none(),
    }
}

/// Drop the partial bytes for `rel` so the retry starts clean instead of
/// resuming onto bytes from another edition.
///
/// The **finished** file is deliberately left alone. It is about to be
/// overwritten by the rename at the end of a successful fetch — which is
/// atomic — and until then it is the only copy the reader has. Deleting it
/// up front buys nothing and turns any later failure into a book that used
/// to work and now doesn't.
async fn discard_partial(dir: &std::path::Path, rel: &str) {
    let _ = tokio::fs::remove_file(dir.join(format!("{rel}.part"))).await;
}

/// What one fetched file yielded: its size on disk.
struct Fetched {
    bytes: i64,
}

/// Stream one file to `{rel}.part`, resuming from any existing bytes via a
/// Range request, then rename to `rel`.
///
/// `on_etag` fires **as soon as the response headers arrive**, before a
/// single body byte is read, so the caller can put the validator on disk
/// while the transfer is still running. That timing is the point: the
/// process can die at any moment during a large download, and the `.part`
/// left behind is only safely resumable if the tag naming the file those
/// bytes came from was already persisted. Reporting it with the result —
/// even on the error path — misses every interruption that isn't a clean
/// error return.
async fn download_file(
    server_url: &str,
    dir: &std::path::Path,
    file: &PlannedFile,
    on_etag: &mut (dyn FnMut(Option<String>) + Send),
    on_delta: &mut (dyn FnMut(i64) + Send),
) -> anyhow::Result<Fetched> {
    let part_path = dir.join(format!("{}.part", file.rel));
    let final_path = dir.join(&file.rel);
    let resumed = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let mut resp = send_ranged_request(server_url, file, resumed).await?;

    // 206 → append after the existing bytes; 200 → the server refused the
    // Range because the file moved (or none was sent), start over.
    let appending = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT && resumed > 0;
    on_etag(
        resp.headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    );
    let expected_total = resp
        .content_length()
        .map(|cl| if appending { resumed + cl } else { cl });

    let written =
        write_body_to_part(&mut resp, &part_path, file, appending, resumed, on_delta).await?;
    if let Some(expected) = expected_total {
        if written != expected as i64 {
            return Err(DownloadError::Interrupted.into());
        }
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .context("Could not finalize the download")?;

    // A byte count proves the transfer ran to length; it does not prove the
    // bytes cohere. Check the container closes before this file is offered
    // to a reader — the one guard that also covers a replacement no
    // stat-derived validator can see (rule 09's known residual).
    if verify::verify(&final_path).await == Verdict::Damaged {
        tracing::warn!(rel = %file.rel, url = %file.url_path, "offline download: assembled file failed container verification");
        // Remove it rather than leave a corrupt book on disk that a retry
        // would try to resume from.
        let _ = tokio::fs::remove_file(&final_path).await;
        return Err(DownloadError::Damaged.into());
    }

    Ok(Fetched { bytes: written })
}

/// Send the (possibly resumed) request for one planned file, mapping
/// transport failures and non-success statuses to a [`DownloadError`].
/// Streaming client: no whole-request timeout, so a large book can't be
/// killed mid-transfer by the default client's 30s cap.
async fn send_ranged_request(
    server_url: &str,
    file: &PlannedFile,
    resumed: u64,
) -> anyhow::Result<reqwest::Response> {
    let url = format!("{server_url}{}", file.url_path);
    let mut req = data::with_bearer(data::streaming_client().get(&url));
    if resumed > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={resumed}-"));
        // Ask the server to honour the Range only if the file is still the
        // one these bytes came from. Without this a replaced file answers
        // 206 and we splice its tail onto the old head — a corrupt book
        // that looks, by every byte count, like a successful download.
        if let Some(etag) = file.http_etag.as_deref() {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(etag) {
                req = req.header(reqwest::header::IF_RANGE, value);
            }
        }
    }
    let resp = req.send().await.map_err(|e| {
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
    Ok(resp)
}

/// Stream `resp`'s body into `{rel}.part`, correcting the progress counter
/// for the `appending`/restart transition first, then return the file's
/// final size on disk.
async fn write_body_to_part(
    resp: &mut reqwest::Response,
    part_path: &std::path::Path,
    file: &PlannedFile,
    appending: bool,
    resumed: u64,
    on_delta: &mut (dyn FnMut(i64) + Send),
) -> anyhow::Result<i64> {
    use tokio::io::AsyncWriteExt;

    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .append(appending)
        .write(true)
        .truncate(!appending)
        .open(part_path)
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

    tokio::fs::metadata(part_path)
        .await
        .map(|m| m.len() as i64)
        .context("Could not verify the downloaded file")
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
        http_etag: None,
        source_etag: None,
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
            http_etag: None,
            source_etag: None,
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

fn publish(
    uuid: &str,
    format: DlFormat,
    title: &str,
    file_id: Option<i64>,
    files: &[PlannedFile],
    downloaded: i64,
    total: Option<i64>,
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
        stale: false,
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
