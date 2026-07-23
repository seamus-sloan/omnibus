//! The download fetch loop: plan the files for a (book, format), stream
//! them to `.part` temp files with Range-based resume, rename on
//! completion, and warm the metadata/manifest/artwork caches so the book
//! is fully usable offline. Runs on the loopback media runtime so
//! downloads survive route changes and component unmounts.

use omnibus_shared::{AudiobookManifest, EbookMetadata, ManifestPart};

use crate::data;
use crate::offline::{cache, media};

use super::{DlFormat, DownloadEntry, DownloadStatus, PlannedFile};

/// Flush registry progress every this many streamed bytes.
const PROGRESS_FLUSH_BYTES: i64 = 1024 * 1024;

/// Run one download to completion (or error). The registry entry was
/// already put in `Downloading` by `downloads::start`.
pub(super) async fn run(server_url: String, uuid: String, format: DlFormat, file_id: Option<i64>) {
    match run_inner(&server_url, &uuid, format, file_id).await {
        Ok(()) => {}
        Err(message) => super::set_error(&uuid, format, message),
    }
}

async fn run_inner(
    server_url: &str,
    uuid: &str,
    format: DlFormat,
    file_id: Option<i64>,
) -> Result<(), String> {
    // The raw `_online` variant: starting a download while offline must
    // fail loudly, never silently plan from a stale cached book.
    let book = data::get_ebook_online(server_url, uuid)
        .await
        .map_err(|e| friendly(&e))?
        .ok_or_else(|| "Book not found".to_string())?;
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
                    return Err("This audiobook's format isn't supported offline".into());
                }
            };
            // The offline player reads the manifest from this cache row.
            cache::put_json(&cache::keys::manifest(uuid, fid), &manifest);
            (plan_audio_parts(&parts), audio_size_estimate(&book))
        }
    };
    if plan.is_empty() {
        return Err("Nothing to download for this format".into());
    }

    let dir = media::downloads_root()
        .map(|r| r.join(uuid))
        .ok_or_else(|| "Offline storage unavailable".to_string())?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Could not create download dir: {e}"))?;

    // Merge prior completion state (resume of a partly-finished download).
    // Stat asynchronously — this future shares the loopback runtime with the
    // media server, so blocking std::fs calls here would stall playback.
    let prior = super::get_entry(uuid, format)
        .map(|e| e.files)
        .unwrap_or_default();
    let mut files: Vec<PlannedFile> = Vec::with_capacity(plan.len());
    for mut f in plan {
        if prior.iter().any(|p| p.rel == f.rel && p.done) {
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
    );

    for idx in 0..files.len() {
        if files[idx].done {
            continue;
        }
        let mut unflushed: i64 = 0;
        let written = download_file(server_url, &dir, &files[idx], &mut |delta| {
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
        files[idx].bytes = Some(written);
        downloaded = files.iter().filter_map(|f| f.bytes).sum();
        publish(
            uuid,
            format,
            &title,
            file_id,
            &files,
            downloaded,
            total_estimate,
        );
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
    });
    Ok(())
}

/// Stream one file to `{rel}.part`, resuming from any existing bytes via a
/// Range request, then rename to `rel`. Returns the final byte count.
async fn download_file(
    server_url: &str,
    dir: &std::path::Path,
    file: &PlannedFile,
    on_delta: &mut (dyn FnMut(i64) + Send),
) -> Result<i64, String> {
    use tokio::io::AsyncWriteExt;

    let part_path = dir.join(format!("{}.part", file.rel));
    let final_path = dir.join(&file.rel);
    let resumed = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let url = format!("{server_url}{}", file.url_path);
    let mut req = data::with_bearer(data::http_client().get(&url));
    if resumed > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={resumed}-"));
    }
    let mut resp = req
        .send()
        .await
        .map_err(|_| "Connection lost".to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Server returned {}", status.as_u16()));
    }

    // 206 → append after the existing bytes; 200 → the server ignored the
    // Range (or none was sent), start over.
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
        .map_err(|e| format!("Could not write download: {e}"))?;
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
            Err(_) => return Err("Connection lost".into()),
        };
        out.write_all(&chunk)
            .await
            .map_err(|e| format!("Could not write download: {e}"))?;
        on_delta(chunk.len() as i64);
    }
    out.flush()
        .await
        .map_err(|e| format!("Could not write download: {e}"))?;
    out.sync_all()
        .await
        .map_err(|e| format!("Could not write download: {e}"))?;
    drop(out);

    let written = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len() as i64)
        .map_err(|e| format!("Could not verify download: {e}"))?;
    if let Some(expected) = expected_total {
        if written != expected as i64 {
            return Err("Download was interrupted — tap to resume".into());
        }
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| format!("Could not finalize download: {e}"))?;
    Ok(written)
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
    });
}

/// Cheap mid-stream progress bump that keeps the existing file list.
fn publish_progress(uuid: &str, format: DlFormat, downloaded: i64, total: Option<i64>) {
    let Some(mut entry) = super::get_entry(uuid, format) else {
        return;
    };
    entry.status = DownloadStatus::Downloading {
        downloaded: downloaded.max(0),
        total,
    };
    entry.updated_at = crate::offline::store::now_secs();
    super::upsert(entry);
}

/// Short, user-facing message for a download failure.
fn friendly(e: &crate::data::DataError) -> String {
    if crate::offline::sync::is_offline_error(e) {
        "You're offline — connect to download".into()
    } else {
        e.to_string()
    }
}
