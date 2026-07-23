//! Book / library / search / settings / overrides / worker fetchers. Each
//! function has a mobile REST variant (`reqwest`) and a web/SSR variant
//! (Dioxus server-function wrapper) with identical signatures across the
//! `#[cfg]` split. Web/SSR wrappers ignore `server_url` because server
//! functions resolve against the page origin.

use omnibus_shared::{
    AudiobookManifest, BookDeletionManifest, DeleteBookFilesResult, EbookLibrary, EbookMetadata,
    LibraryContents, LibraryPage, MergeBooksResult, MetadataOverrides, PaletteResults, Settings,
    SortDir, SortKey, ViewFilters, WorkerStatus,
};

#[cfg(not(feature = "mobile"))]
use omnibus_shared::MetadataSource;

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/settings` — fetch library paths and indexer config.
#[cfg(feature = "mobile")]
pub async fn get_settings(server_url: &str) -> Result<Settings, DataError> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Settings>().await?)
}

/// POST `/api/settings` — persist updated library paths; server kicks a reindex.
#[cfg(feature = "mobile")]
pub async fn save_settings(server_url: &str, settings: Settings) -> Result<Settings, DataError> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().post(&url).json(&settings))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Settings>().await?)
}

/// GET `/api/library` — fetch the high-level library section listing.
/// Cache-first with background revalidation.
#[cfg(feature = "mobile")]
pub async fn get_library(server_url: &str) -> Result<LibraryContents, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through(crate::offline::cache::keys::library(), async move {
        get_library_online(&url).await
    })
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_library_online(server_url: &str) -> Result<LibraryContents, DataError> {
    let url = format!("{server_url}/api/library");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<LibraryContents>().await?)
}

/// GET `/api/ebooks` — fetch the full ebook library payload.
/// Cache-first with background revalidation.
#[cfg(feature = "mobile")]
pub async fn get_ebooks(server_url: &str) -> Result<EbookLibrary, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through("ebooks".to_string(), async move {
        get_ebooks_online(&url).await
    })
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_ebooks_online(server_url: &str) -> Result<EbookLibrary, DataError> {
    let url = format!("{server_url}/api/ebooks");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<EbookLibrary>().await?)
}

/// GET `/api/ebooks?sort=&dir=&cursor=&limit=&formats=` — one keyset page
/// (F5b).
///
/// Of the sidebar facets only `filters.formats` rides the REST query (the
/// mobile Sort & filter sheet's chips); the rest are ignored and `facets`
/// comes back `None` (a web concern). `total` is read from `X-Total-Count`
/// on the first page only; `next_cursor` from `X-Next-Cursor`.
#[cfg(feature = "mobile")]
pub async fn get_ebooks_page(
    server_url: &str,
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: ViewFilters,
    cursor: Option<String>,
    limit: i64,
) -> Result<LibraryPage, DataError> {
    if crate::offline::sync::is_offline() {
        // Known-offline fast path: serve the full replica directly — its
        // `off:` cursors keep pagination consistent with zero network.
        return crate::offline::replica::page_from_cache(
            sort_key,
            sort_dir,
            &filters,
            cursor.as_deref(),
            limit,
        )
        .await
        .ok_or(DataError::Offline);
    }
    // Replica freshness no longer rides the landing fetch outcome — kick
    // the TTL-gated background sync on every online browse.
    crate::offline::replica::ensure_fresh(server_url.to_string());
    if cursor.is_none() {
        // First page: cache-first via the SWR policy, so landing paints
        // instantly (server-exact ordering as of the last visit) and
        // revalidates in the background.
        let key = crate::offline::cache::keys::ebooks_first(
            sort_key.as_wire(),
            sort_dir.as_wire(),
            &filters.formats.join(","),
        );
        let url = server_url.to_string();
        let f = filters.clone();
        let result = crate::offline::cache::read_through(key, async move {
            get_ebooks_page_online(&url, sort_key, sort_dir, f, None, limit).await
        })
        .await;
        return match result {
            // Went offline mid-request with a cold first-page cache — the
            // full replica may still save the paint.
            Err(e) if crate::offline::sync::is_offline_error(&e) => {
                crate::offline::replica::page_from_cache(sort_key, sort_dir, &filters, None, limit)
                    .await
                    .ok_or(e)
            }
            r => r,
        };
    }
    // Cursor pages ("load more"): network-first with replica fallback.
    let attempt = get_ebooks_page_online(
        server_url,
        sort_key,
        sort_dir,
        filters.clone(),
        cursor.clone(),
        limit,
    )
    .await;
    match attempt {
        Ok(page) => {
            crate::offline::sync::note_online();
            Ok(page)
        }
        Err(e) if crate::offline::sync::is_offline_error(&e) => {
            crate::offline::sync::note_offline();
            match crate::offline::replica::page_from_cache(
                sort_key,
                sort_dir,
                &filters,
                cursor.as_deref(),
                limit,
            )
            .await
            {
                Some(page) => Ok(page),
                None => Err(e),
            }
        }
        Err(e) => {
            crate::offline::sync::note_online();
            Err(e)
        }
    }
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_ebooks_page_online(
    server_url: &str,
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: ViewFilters,
    cursor: Option<String>,
    limit: i64,
) -> Result<LibraryPage, DataError> {
    let mut url = format!(
        "{server_url}/api/ebooks?sort={}&dir={}&limit={limit}",
        sort_key.as_wire(),
        sort_dir.as_wire(),
    );
    if !filters.formats.is_empty() {
        // Format keys are plain lowercase tokens (`epub`, `m4b`) — no
        // URL-encoding needed.
        url.push_str("&formats=");
        url.push_str(&filters.formats.join(","));
    }
    if let Some(c) = &cursor {
        url.push_str("&cursor=");
        url.push_str(c);
    }
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    // Read headers before `.json()` consumes the response.
    let next_cursor = response
        .headers()
        .get("X-Next-Cursor")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let total = response
        .headers()
        .get("X-Total-Count")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());
    let lib = response.json::<EbookLibrary>().await?;
    Ok(LibraryPage {
        path: lib.path,
        books: lib.books,
        next_cursor,
        // Surface the total only on the first page, matching the web RPC shape.
        total: cursor.is_none().then_some(total).flatten(),
        facets: None,
    })
}

/// GET `/api/search?q=` — full-text search across the ebook library.
/// Offline, degrades to a substring match over the local library replica.
#[cfg(feature = "mobile")]
pub async fn search_ebooks(server_url: &str, q: &str) -> Result<EbookLibrary, DataError> {
    if crate::offline::sync::is_offline() {
        // Known-offline fast path: substring match over the replica, no
        // doomed network attempt. Online search stays network-first —
        // search wants fresh data and per-query caching would bloat the
        // store.
        return crate::offline::replica::search_from_cache(q)
            .await
            .ok_or(DataError::Offline);
    }
    match search_ebooks_online(server_url, q).await {
        Ok(lib) => {
            crate::offline::sync::note_online();
            Ok(lib)
        }
        Err(e) if crate::offline::sync::is_offline_error(&e) => {
            crate::offline::sync::note_offline();
            match crate::offline::replica::search_from_cache(q).await {
                Some(lib) => Ok(lib),
                None => Err(e),
            }
        }
        Err(e) => {
            crate::offline::sync::note_online();
            Err(e)
        }
    }
}

#[cfg(feature = "mobile")]
pub(crate) async fn search_ebooks_online(
    server_url: &str,
    q: &str,
) -> Result<EbookLibrary, DataError> {
    // Percent-encode the query so FTS5 operators and whitespace survive the
    // URL.
    let encoded: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{server_url}/api/search?q={encoded}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<EbookLibrary>().await?)
}

/// Search palette — grouped results for the command-palette overlay.
#[cfg(feature = "mobile")]
pub async fn search_palette(server_url: &str, q: &str) -> Result<PaletteResults, DataError> {
    crate::data::require_online()?;
    let encoded: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{server_url}/api/search/palette?q={encoded}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<PaletteResults>().await?)
}

/// GET `/api/ebooks/{uuid}` — fetch one ebook by uuid, `Ok(None)` on 404.
/// Cache-first with background revalidation.
#[cfg(feature = "mobile")]
pub async fn get_ebook(server_url: &str, uuid: &str) -> Result<Option<EbookMetadata>, DataError> {
    let url = server_url.to_string();
    let uuid = uuid.to_string();
    crate::offline::cache::read_through(crate::offline::cache::keys::ebook(&uuid), async move {
        get_ebook_online(&url, &uuid).await
    })
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_ebook_online(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// GET `/api/audiobooks/{uuid}/manifest` — fetch the direct-play / HLS
/// manifest driving the mobile player. `file_id` targets a specific
/// `book_files` row for multi-file audiobooks.
#[cfg(feature = "mobile")]
pub async fn get_manifest(
    server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<AudiobookManifest, DataError> {
    let url = server_url.to_string();
    let uuid = uuid.to_string();
    crate::offline::cache::read_through(
        crate::offline::cache::keys::manifest(&uuid, file_id),
        async move { get_manifest_online(&url, &uuid, file_id).await },
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_manifest_online(
    server_url: &str,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<AudiobookManifest, DataError> {
    let url = match file_id {
        Some(fid) => format!("{server_url}/api/audiobooks/{uuid}/manifest?file_id={fid}"),
        None => format!("{server_url}/api/audiobooks/{uuid}/manifest"),
    };
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<AudiobookManifest>().await?)
}

/// Web/SSR `get_manifest` — the web player fetches its manifest through the
/// `bootstrap` JS shim, so this stub only exists to keep the signature
/// parallel across the `#[cfg]` split; it is never called on web/SSR.
#[cfg(not(feature = "mobile"))]
pub async fn get_manifest(
    _server_url: &str,
    _uuid: &str,
    _file_id: Option<i64>,
) -> Result<AudiobookManifest, DataError> {
    Err(DataError::Other(
        "get_manifest is mobile-only; web uses the bootstrap shim".into(),
    ))
}

/// POST `/api/ebooks/{uuid}/overrides` — persist user metadata overrides.
#[cfg(feature = "mobile")]
pub async fn save_overrides(
    server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().post(&url))
        .json(overrides)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// DELETE `/api/ebooks/{uuid}/overrides` — revert to original metadata.
#[cfg(feature = "mobile")]
pub async fn delete_overrides(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// Multipart upload of a replacement cover for a book (mobile). Mirrors
/// `upload_author_photo`'s mobile REST path against the analogous
/// `/api/ebooks/:uuid/cover` endpoint.
#[cfg(feature = "mobile")]
pub async fn upload_ebook_cover(
    server_url: &str,
    uuid: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let endpoint = format!("{server_url}/api/ebooks/{uuid}/cover");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime)?;
    let form = reqwest::multipart::Form::new().part("cover", part);
    let response = with_bearer(http_client().post(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// DELETE `/api/ebooks/{uuid}/cover` — revert an overridden cover to the
/// scanned original, preserving any other field overrides.
#[cfg(feature = "mobile")]
pub async fn delete_ebook_cover(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/ebooks/{uuid}/cover");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

/// Web/SSR `get_settings` — server-function wrapper that proxies to `rpc_get_settings`.
#[cfg(not(feature = "mobile"))]
pub async fn get_settings(_server_url: &str) -> Result<Settings, DataError> {
    crate::rpc::rpc_get_settings()
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `save_settings` — server-function wrapper that proxies to `rpc_save_settings`.
#[cfg(not(feature = "mobile"))]
pub async fn save_settings(_server_url: &str, settings: Settings) -> Result<Settings, DataError> {
    crate::rpc::rpc_save_settings(settings)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_library` — server-function wrapper that proxies to `rpc_get_library`.
#[cfg(not(feature = "mobile"))]
pub async fn get_library(_server_url: &str) -> Result<LibraryContents, DataError> {
    crate::rpc::rpc_get_library()
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR: fetch the metadata-source precedence for `library` (`"ebook"` or
/// `"audiobook"`, F5.1 #972) — server-function wrapper that proxies to
/// `rpc_get_metadata_precedence`. Web-only for now (no mobile REST route);
/// mobile keeps the today's-effective-order behavior until a mobile editing
/// surface is built.
#[cfg(not(feature = "mobile"))]
pub async fn get_metadata_precedence(
    _server_url: &str,
    library: &str,
) -> Result<Vec<MetadataSource>, DataError> {
    crate::rpc::rpc_get_metadata_precedence(library.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR: persist the metadata-source precedence for `library` — proxies
/// to `rpc_set_metadata_precedence`. Returns the saved order.
#[cfg(not(feature = "mobile"))]
pub async fn save_metadata_precedence(
    _server_url: &str,
    library: &str,
    precedence: Vec<MetadataSource>,
) -> Result<Vec<MetadataSource>, DataError> {
    crate::rpc::rpc_set_metadata_precedence(library.to_string(), precedence)
        .await
        .map_err(note_server_fn_err)
}

/// Snapshot of the worker progress feed. Web calls the RPC; mobile returns
/// an empty status because the corresponding REST endpoint doesn't exist
/// yet — the stub keeps callers' types lined up across feature gates.
#[cfg(not(feature = "mobile"))]
pub async fn worker_status(_server_url: &str) -> Result<WorkerStatus, DataError> {
    crate::rpc::rpc_worker_status()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `worker_status` — returns an empty snapshot until the REST mirror lands.
#[cfg(feature = "mobile")]
pub async fn worker_status(_server_url: &str) -> Result<WorkerStatus, DataError> {
    // Mobile REST mirror is a follow-up; return an empty status so any
    // future mobile caller compiles against the same signature the web
    // build uses.
    Ok(WorkerStatus::default())
}

/// Admin: manually trigger a rescan of the configured library paths.
#[cfg(not(feature = "mobile"))]
pub async fn scan_library(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_scan_library()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `scan_library` — queues both library scans via REST.
#[cfg(feature = "mobile")]
pub async fn scan_library(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/scan-library");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Admin: manually trigger chapter extraction for audiobooks missing chapters.
#[cfg(not(feature = "mobile"))]
pub async fn backfill_chapters(_server_url: &str) -> Result<(), DataError> {
    crate::rpc::rpc_backfill_chapters()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `backfill_chapters`.
#[cfg(feature = "mobile")]
pub async fn backfill_chapters(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/audiobooks/backfill-chapters");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Admin: merge the book `source_uuid` into `target_uuid`. Web-only —
/// mobile has no admin surface.
#[cfg(not(feature = "mobile"))]
pub async fn merge_books(
    _server_url: &str,
    source_uuid: &str,
    target_uuid: &str,
) -> Result<MergeBooksResult, DataError> {
    crate::rpc::rpc_merge_books(source_uuid.to_string(), target_uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `merge_books` — merge is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn merge_books(
    _server_url: &str,
    _source_uuid: &str,
    _target_uuid: &str,
) -> Result<MergeBooksResult, DataError> {
    Err(DataError::Other("merge is web-only".into()))
}

/// Admin: candidate search for the merge dialog — FTS across both
/// configured libraries (unlike `search_ebooks`, which is ebook-only).
#[cfg(not(feature = "mobile"))]
pub async fn merge_candidates(_server_url: &str, q: &str) -> Result<Vec<EbookMetadata>, DataError> {
    crate::rpc::rpc_merge_candidates(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `merge_candidates` — merge is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn merge_candidates(
    _server_url: &str,
    _q: &str,
) -> Result<Vec<EbookMetadata>, DataError> {
    Err(DataError::Other("merge is web-only".into()))
}

/// Admin: undo a merge by its `merge_log` id. Returns the restored uuid.
#[cfg(not(feature = "mobile"))]
pub async fn undo_merge(_server_url: &str, merge_log_id: i64) -> Result<String, DataError> {
    crate::rpc::rpc_undo_merge(merge_log_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `undo_merge` — merge is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn undo_merge(_server_url: &str, _merge_log_id: i64) -> Result<String, DataError> {
    Err(DataError::Other("merge is web-only".into()))
}

/// Admin: the book's deletable items plus the user data a total delete would
/// take with them. Web-only — mobile has no admin surface.
#[cfg(not(feature = "mobile"))]
pub async fn book_deletion_manifest(
    _server_url: &str,
    uuid: &str,
) -> Result<BookDeletionManifest, DataError> {
    crate::rpc::rpc_book_deletion_manifest(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `book_deletion_manifest` — deletion is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn book_deletion_manifest(
    _server_url: &str,
    _uuid: &str,
) -> Result<BookDeletionManifest, DataError> {
    Err(DataError::Other("deletion is web-only".into()))
}

/// Admin: delete the given files and physical copies; when no item is left,
/// the book goes too.
#[cfg(not(feature = "mobile"))]
pub async fn delete_book_files(
    _server_url: &str,
    uuid: &str,
    file_ids: Vec<i64>,
    copy_ids: Vec<i64>,
) -> Result<DeleteBookFilesResult, DataError> {
    crate::rpc::rpc_delete_book_files(uuid.to_string(), file_ids, copy_ids)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub for `delete_book_files` — deletion is a web-admin-only surface.
#[cfg(feature = "mobile")]
pub async fn delete_book_files(
    _server_url: &str,
    _uuid: &str,
    _file_ids: Vec<i64>,
    _copy_ids: Vec<i64>,
) -> Result<DeleteBookFilesResult, DataError> {
    Err(DataError::Other("deletion is web-only".into()))
}

/// Web/SSR `get_ebooks` — server-function wrapper that proxies to `rpc_get_ebooks`.
#[cfg(not(feature = "mobile"))]
pub async fn get_ebooks(_server_url: &str) -> Result<EbookLibrary, DataError> {
    crate::rpc::rpc_get_ebooks()
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_ebooks_page` — one keyset page (F5b) via `rpc_get_ebooks_page`.
/// `server_url` is unused (server functions resolve against the page origin).
#[cfg(not(feature = "mobile"))]
pub async fn get_ebooks_page(
    _server_url: &str,
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: ViewFilters,
    cursor: Option<String>,
    limit: i64,
) -> Result<LibraryPage, DataError> {
    crate::rpc::rpc_get_ebooks_page(sort_key, sort_dir, filters, cursor, limit)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `search_ebooks` — server-function wrapper that proxies to `rpc_search`.
#[cfg(not(feature = "mobile"))]
pub async fn search_ebooks(_server_url: &str, q: &str) -> Result<EbookLibrary, DataError> {
    crate::rpc::rpc_search(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Search palette — grouped results for the command-palette overlay.
#[cfg(not(feature = "mobile"))]
pub async fn search_palette(_server_url: &str, q: &str) -> Result<PaletteResults, DataError> {
    crate::rpc::rpc_search_palette(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_ebook` — server-function wrapper that proxies to `rpc_get_ebook`.
#[cfg(not(feature = "mobile"))]
pub async fn get_ebook(_server_url: &str, uuid: &str) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_get_ebook(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `save_overrides` — server-function wrapper that proxies to `rpc_save_overrides`.
#[cfg(not(feature = "mobile"))]
pub async fn save_overrides(
    _server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_save_overrides(uuid.to_string(), overrides.clone())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `delete_overrides` — server-function wrapper that proxies to `rpc_delete_overrides`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_overrides(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_delete_overrides(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Multipart upload of a replacement cover on the web client.
///
/// Server functions can't carry binary file uploads (they JSON-serialize
/// their arguments), so this bypasses RPC and POSTs directly to the REST
/// endpoint via `gloo-net`, mirroring `upload_author_photo`'s web path.
#[cfg(feature = "web")]
pub async fn upload_ebook_cover(
    _server_url: &str,
    uuid: &str,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<Option<EbookMetadata>, DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let endpoint = format!("/api/ebooks/{uuid}/cover");
    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    // See `upload_author_photo`'s web impl for why this goes through a
    // one-element `Uint8Array` → `Array` → `Blob` rather than a direct
    // byte-slice constructor.
    let u8 = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(&mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))?;
    form.append_with_blob_and_filename("cover", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::post(&endpoint)
        // Don't set Content-Type: the browser fills it in with the
        // multipart boundary.
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if res.status() == 401 {
        super::web_auth_state::notify_unauthorized();
        return Err(DataError::Unauthorized);
    }
    if !res.ok() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(DataError::Http { status, body });
    }
    let book = res
        .json::<EbookMetadata>()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    Ok(Some(book))
}

/// Fallback stub for the non-web, non-mobile build (cargo check on the
/// default workspace members compiles the frontend with no platform
/// feature so type-checking still passes). The metadata edit sidebar only
/// invokes upload after a user `onchange`, which never fires under SSR.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_ebook_cover(
    _server_url: &str,
    _uuid: &str,
    _filename: String,
    _mime: String,
    _bytes: Vec<u8>,
) -> Result<Option<EbookMetadata>, DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

/// Web/SSR `delete_ebook_cover` — server-function wrapper that proxies to
/// `rpc_delete_ebook_cover`.
#[cfg(not(feature = "mobile"))]
pub async fn delete_ebook_cover(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_delete_ebook_cover(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

#[cfg(all(test, feature = "mobile"))]
mod tests;
