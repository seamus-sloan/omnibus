//! Browse, search, per-book, manifest, and download-validator fetchers —
//! the read side of the books domain, including the offline cache/replica
//! policies the mobile client layers over each read.

use omnibus_shared::{
    AudiobookManifest, EbookLibrary, EbookMetadata, LibraryPage, PaletteResults, SortDir, SortKey,
    ViewFilters,
};

#[cfg(not(feature = "mobile"))]
use crate::data::note_server_fn_err;
use crate::data::DataError;
#[cfg(feature = "mobile")]
use crate::data::{drain_error, http_client, note_status, with_bearer};

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

/// Pull-to-refresh: force the first browse page through a server round
/// trip, landing it in the SWR cache — and bumping the cache generation
/// when it changed — so every subscribed screen refetches. Awaitable so
/// the refresh indicator can track the real network round trip.
#[cfg(feature = "mobile")]
pub async fn refresh_ebooks_first_page(
    server_url: &str,
    sort_key: SortKey,
    sort_dir: SortDir,
    filters: ViewFilters,
    limit: i64,
) {
    let key = crate::offline::cache::keys::ebooks_first(
        sort_key.as_wire(),
        sort_dir.as_wire(),
        &filters.formats.join(","),
    );
    let url = server_url.to_string();
    crate::offline::cache::refresh(key, async move {
        get_ebooks_page_online(&url, sort_key, sort_dir, filters, None, limit).await
    })
    .await;
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
    let book = crate::offline::cache::read_through(
        crate::offline::cache::keys::ebook(&uuid),
        async move { get_ebook_online(&url, &uuid).await },
    )
    .await?;
    // This is the only read that carries per-file content validators, so
    // it's where a downloaded copy learns the library file moved under it.
    if let Some(book) = book.as_ref() {
        crate::offline::downloads::note_book(book);
    }
    Ok(book)
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

/// `POST /api/downloads/validators` — current validators for a batch of
/// downloaded files, in one request carrying no metadata.
///
/// Deliberately not cached: the answer's whole purpose is to be newer than
/// what this device holds, and it is small enough that a cache row would
/// cost more than the request.
#[cfg(feature = "mobile")]
pub async fn download_validators(
    server_url: &str,
    files: &[omnibus_shared::DownloadValidatorQuery],
) -> Result<Vec<omnibus_shared::DownloadValidator>, DataError> {
    let url = format!("{server_url}/api/downloads/validators");
    let body = omnibus_shared::DownloadValidatorRequest {
        files: files.to_vec(),
    };
    let response = with_bearer(http_client().post(&url))
        .json(&body)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response
        .json::<omnibus_shared::DownloadValidatorResponse>()
        .await?
        .files)
}
