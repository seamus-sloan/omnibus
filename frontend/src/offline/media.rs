//! Loopback media server for offline playback: a tiny axum server on
//! `127.0.0.1:{random port}` that serves downloaded book files (with Range
//! support, so `<audio>` can seek) and an offline image cache that proxies
//! covers/thumbs from the real server while online. Guarded by a per-boot
//! random `?token=` — the same query-token pattern the real server's media
//! routes use, since WebView subresource fetches can't carry a bearer
//! header. Every response carries `Access-Control-Allow-Origin: *` because
//! the WebView origin is wry's custom scheme (`dioxus://` on iOS), exactly
//! mirroring the server's own `/api/ebooks/{uuid}/file` CORS behavior.
//! Background ETag revalidation of the cached images lives in
//! [`revalidate`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use omnibus_shared::http_range::{self as range, RangeOutcome};

mod revalidate;

use revalidate::{lock_cache_key, Fetched};

/// Where the running loopback server can be reached this boot.
pub struct LoopbackInfo {
    pub port: u16,
    pub token: String,
}

/// Soft cap for the proxied-image disk cache; pruned oldest-first at boot.
const IMG_CACHE_CAP_BYTES: u64 = 256 * 1024 * 1024;

/// How stale a cached image may be before the proxy checks back with the
/// server. Covers change rarely and revalidation is a background 304, but
/// without a window a grid scroll would fire one conditional request per
/// visible cover every time it came into view. Five minutes keeps a cover
/// edited on another device arriving promptly while leaving scrolling free;
/// the `?v=` cache-bust remains the immediate path for an edit made here.
const IMG_REVALIDATE_AFTER_SECS: u64 = 300;

static LOOPBACK: OnceLock<Option<LoopbackInfo>> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Upstream server base URL for the image proxy's online fetches. Kept in a
/// plain lock (not the Dioxus signal) because the axum handlers run off the
/// UI thread; refreshed by [`set_upstream`] whenever a media URL is built.
fn upstream() -> &'static RwLock<String> {
    static UPSTREAM: OnceLock<RwLock<String>> = OnceLock::new();
    UPSTREAM.get_or_init(|| RwLock::new(String::new()))
}

/// Record the current server base URL for proxy upstream fetches.
pub fn set_upstream(server_url: &str) {
    if server_url.is_empty() {
        return;
    }
    if let Ok(mut guard) = upstream().write() {
        if *guard != server_url {
            *guard = server_url.to_string();
        }
    }
}

fn upstream_url() -> Option<String> {
    let guard = upstream().read().ok()?;
    (!guard.is_empty()).then(|| guard.clone())
}

/// The running loopback server, or `None` when it failed to start (no
/// writable data dir) — callers then fall back to tokened server URLs.
pub fn loopback() -> Option<&'static LoopbackInfo> {
    LOOPBACK.get().and_then(|l| l.as_ref())
}

/// `true` once the loopback runtime exists — lets callers with their own
/// fallback runtime decide before handing a future to [`spawn_on_runtime`]
/// (which consumes it either way).
pub(crate) fn runtime_available() -> bool {
    RUNTIME.get().is_some()
}

/// Spawn `fut` on the loopback server's tokio runtime — the offline layer's
/// home for work that must outlive any Dioxus component scope (downloads
/// keep running across route changes).
pub(crate) fn spawn_on_runtime<F>(fut: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    match RUNTIME.get() {
        Some(handle) => {
            handle.spawn(fut);
            true
        }
        None => false,
    }
}

/// Root dir for downloaded book files: `{data_dir}/downloads`.
pub(crate) fn downloads_root() -> Option<PathBuf> {
    crate::data::app_dirs::data_dir().map(|d| d.join("downloads"))
}

/// Root dir for the proxied-image cache: `{data_dir}/imgcache`.
pub(crate) fn imgcache_root() -> Option<PathBuf> {
    crate::data::app_dirs::data_dir().map(|d| d.join("imgcache"))
}

/// Start the loopback server. Idempotent; on any failure the offline layer
/// degrades to online-only playback (tokened server URLs keep working).
pub fn start() {
    LOOPBACK.get_or_init(|| {
        let root = downloads_root()?;
        let img_dir = imgcache_root()?;
        for dir in [&root, &img_dir] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!(error = %e, path = %dir.display(), "could not create offline media dir");
                return None;
            }
        }
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "could not bind loopback media server");
                return None;
            }
        };
        let port = listener.local_addr().ok()?.port();
        let token = fresh_token()?;
        let state = Arc::new(MediaState {
            root,
            img_dir,
            token: token.clone(),
        });
        if std::thread::Builder::new()
            .name("omnibus-media-server".into())
            .spawn(move || serve_thread(listener, state))
            .is_err()
        {
            return None;
        }
        Some(LoopbackInfo { port, token })
    });
}

/// Per-boot random token (32 hex chars). A new one every launch keeps a URL
/// that leaked out of the app (logs, screenshots) from working later.
fn fresh_token() -> Option<String> {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn serve_thread(listener: std::net::TcpListener, state: Arc<MediaState>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::warn!(error = %e, "could not build loopback media runtime");
            return;
        }
    };
    let _ = RUNTIME.set(rt.handle().clone());
    rt.block_on(async move {
        prune_img_cache(&state.img_dir, IMG_CACHE_CAP_BYTES);
        if listener.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
            return;
        };
        let app = router(state);
        if let Err(e) = axum::serve(listener, app).await {
            tracing::warn!(error = %e, "loopback media server exited");
        }
    });
}

/// Build a loopback URL for a downloaded file (`/dl/{uuid}/{rel}`), or
/// `None` when the server isn't running.
pub(crate) fn local_file_url(uuid: &str, rel: &str) -> Option<String> {
    let info = loopback()?;
    Some(format!(
        "http://127.0.0.1:{}/dl/{uuid}/{rel}?token={}",
        info.port, info.token
    ))
}

/// Loopback image-proxy URL for a media path (`/api/thumbs/…`), or `None`
/// when the server isn't running. Records `server_url` as the proxy's
/// upstream so cache misses can fetch while online.
pub fn proxy_img_url(server_url: &str, path: &str) -> Option<String> {
    if !proxy_path_allowed(path) {
        return None;
    }
    set_upstream(server_url);
    let info = loopback()?;
    let encoded = urlencode(path);
    Some(format!(
        "http://127.0.0.1:{}/img?path={encoded}&token={}",
        info.port, info.token
    ))
}

/// Fetch one image through the proxy's cache path (used by the download
/// engine to guarantee a downloaded book's artwork is available offline).
pub(crate) async fn warm_image(server_url: &str, path: &str) {
    set_upstream(server_url);
    let Some(img_dir) = imgcache_root() else {
        return;
    };
    if !proxy_path_allowed(path) {
        return;
    }
    if cached_image(&img_dir, path).await.is_some() {
        return;
    }
    let _ = fetch_and_cache(&img_dir, path, None).await;
}

struct MediaState {
    root: PathBuf,
    img_dir: PathBuf,
    token: String,
}

fn router(state: Arc<MediaState>) -> Router {
    Router::new()
        .route("/dl/{uuid}/{file}", get(dl_handler))
        .route("/img", get(img_handler))
        .with_state(state)
}

#[derive(serde::Deserialize)]
struct DlQuery {
    token: Option<String>,
}

#[derive(serde::Deserialize)]
struct ImgQuery {
    path: Option<String>,
    token: Option<String>,
    /// Cover cache-bust counter (`contexts::append_cache_bust`); any nonzero
    /// value bypasses the disk cache so an edited cover refreshes.
    v: Option<u32>,
}

async fn dl_handler(
    State(st): State<Arc<MediaState>>,
    UrlPath((uuid, file)): UrlPath<(String, String)>,
    Query(q): Query<DlQuery>,
    headers: HeaderMap,
) -> Response {
    if q.token.as_deref() != Some(st.token.as_str()) {
        return plain(StatusCode::FORBIDDEN, "bad token");
    }
    if !sanitize_segment(&uuid) || !sanitize_segment(&file) {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    let path = st.root.join(&uuid).join(&file);
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    serve_file(&path, ext_mime(&file), range_header.as_deref()).await
}

async fn img_handler(
    State(st): State<Arc<MediaState>>,
    Query(q): Query<ImgQuery>,
    headers: HeaderMap,
) -> Response {
    if q.token.as_deref() != Some(st.token.as_str()) {
        return plain(StatusCode::FORBIDDEN, "bad token");
    }
    let Some(path) = q.path.filter(|p| proxy_path_allowed(p)) else {
        return plain(StatusCode::NOT_FOUND, "not found");
    };
    let bust = q.v.unwrap_or(0) > 0;
    let cached = cached_image(&st.img_dir, &path).await;
    if !bust {
        if let Some(hit) = cached {
            // Answer from disk immediately — as a 304 when the WebView
            // already holds these bytes — then ask the server whether they
            // are still current. A cover replaced from another device lands
            // on the next render rather than costing this one a round-trip.
            revalidate::revalidate_in_background(st.img_dir.clone(), path.clone(), &hit);
            return img_answer(&headers, hit.bytes, &hit.content_type);
        }
    }
    // An explicit `?v=` bust still revalidates rather than blindly
    // refetching: offering the stored validator turns "the cover I just
    // edited" into a full body and "nothing actually changed" into a 304.
    let if_none_match = cached.as_ref().and_then(|c| c.etag.clone());
    // Same lock a background revalidation takes, so the two can't write the
    // same entry at once — and so this write, being the explicit one, is
    // never overwritten by an answer describing the pre-edit cover.
    let guard = lock_cache_key(&path).await;
    let fetched = fetch_and_cache(&st.img_dir, &path, if_none_match.as_deref()).await;
    drop(guard);
    if let Fetched::Replaced {
        bytes,
        content_type,
    } = fetched
    {
        return img_answer(&headers, bytes, &content_type);
    }
    // `Unchanged` means the cached copy is confirmed current; `Failed`
    // means offline or an answer we won't cache, and a stale image still
    // beats a broken one. Either way the disk copy is what to serve — and
    // an uncached thumb falls back to the cached full cover, since thumbs
    // 202 until generated and a download may have only warmed the cover.
    let fallback = match cached {
        Some(hit) => Some(hit),
        None => match thumb_cover_fallback(&path) {
            Some(cover) => cached_image(&st.img_dir, &cover).await,
            None => None,
        },
    };
    match fallback {
        Some(hit) => img_answer(&headers, hit.bytes, &hit.content_type),
        None => plain(StatusCode::NOT_FOUND, "not cached"),
    }
}

/// Map an uncached `/api/thumbs/{uuid}/{size}` request to its book's
/// full-cover path.
fn thumb_cover_fallback(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/api/thumbs/")?;
    let uuid = rest.split('/').next().filter(|u| !u.is_empty())?;
    Some(format!("/api/covers/{uuid}"))
}

/// Serve `path` from disk honoring a single-range request, streaming the
/// body so multi-hundred-MB audiobooks never sit in memory.
async fn serve_file(path: &Path, mime: &'static str, range_header: Option<&str>) -> Response {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return plain(StatusCode::NOT_FOUND, "not found");
    };
    let Ok(meta) = file.metadata().await else {
        return plain(StatusCode::NOT_FOUND, "not found");
    };
    let len = meta.len();
    match range::parse(range_header, len) {
        RangeOutcome::Full => {
            let stream = ReaderStream::new(file);
            base_response(StatusCode::OK, mime, len)
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response"))
        }
        RangeOutcome::Partial { start, end } => {
            if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return plain(StatusCode::INTERNAL_SERVER_ERROR, "seek");
            }
            let span = end - start + 1;
            let stream = ReaderStream::new(file.take(span));
            base_response(StatusCode::PARTIAL_CONTENT, mime, span)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response"))
        }
        RangeOutcome::NotSatisfiable => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .body(Body::empty())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response")),
    }
}

fn base_response(
    status: StatusCode,
    mime: &'static str,
    content_length: u64,
) -> axum::http::response::Builder {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, content_length)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
}

/// `Cache-Control` for every loopback image response.
///
/// **Not** a `max-age`. The WebView honours one, and a day-long one meant
/// re-rendering a cover never reached this handler at all — so the
/// revalidation below never ran, and on the rare render that did reach it
/// the *old* bytes went back with another day of freshness. A cover
/// replaced elsewhere could take two days or an app restart to appear.
///
/// `no-cache` makes the WebView ask every time, which costs a loopback
/// round-trip and, thanks to the `ETag`, usually a bodyless 304 — the
/// WebView keeps its decoded image. Real network traffic is still governed
/// by [`IMG_REVALIDATE_AFTER_SECS`].
const IMG_CACHE_CONTROL: &str = "private, no-cache";

/// Content-derived `ETag` for a loopback image response, so the WebView's
/// conditional re-render is answered with a 304 instead of the bytes.
///
/// Derived from the bytes rather than the upstream validator: it has to
/// exist even for an entry cached before validators, or from a server that
/// publishes none. Not stable across toolchain versions (`DefaultHasher`),
/// which costs one extra transfer per image after an upgrade and nothing
/// else — the same trade `backend::covers` makes.
fn img_etag(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

fn img_response(bytes: Vec<u8>, content_type: &str) -> Response {
    let etag = img_etag(&bytes);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, IMG_CACHE_CONTROL)
        .header(header::ETAG, etag)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response"))
}

/// The bodyless answer to a WebView re-render of an unchanged image.
fn img_not_modified(etag: &str) -> Response {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CACHE_CONTROL, IMG_CACHE_CONTROL)
        .header(header::ETAG, etag)
        .body(Body::empty())
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response"))
}

/// Serve `bytes` as 200, or 304 when the WebView already holds them.
fn img_answer(request: &HeaderMap, bytes: Vec<u8>, content_type: &str) -> Response {
    let etag = img_etag(&bytes);
    let matches = request
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|c| c.trim().trim_start_matches("W/") == etag)
        });
    if matches {
        return img_not_modified(&etag);
    }
    img_response(bytes, content_type)
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Body::from(msg))
        .unwrap_or_default()
}

/// A disk-cache hit: the bytes, the content type recorded alongside them,
/// and the `ETag` the server published for them (absent for entries cached
/// before validators existed, or by a server that sent none).
struct CachedImage {
    bytes: Vec<u8>,
    content_type: String,
    etag: Option<String>,
    /// How long ago these bytes were written, in seconds.
    age_secs: u64,
}

/// Read a cached image + its recorded content type and validator.
async fn cached_image(img_dir: &Path, path: &str) -> Option<CachedImage> {
    let name = cache_file_name(path);
    let file = img_dir.join(&name);
    let bytes = tokio::fs::read(&file).await.ok()?;
    let content_type = tokio::fs::read_to_string(img_dir.join(format!("{name}.ct")))
        .await
        .unwrap_or_else(|_| "image/jpeg".into());
    let etag = tokio::fs::read_to_string(img_dir.join(format!("{name}.etag")))
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let age_secs = tokio::fs::metadata(&file)
        .await
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX);
    Some(CachedImage {
        bytes,
        content_type,
        etag,
        age_secs,
    })
}

/// Restart a cached entry's freshness window without rewriting its bytes.
///
/// A 304 is the server confirming the copy on disk is current, which is
/// exactly as good as having refetched it. Leaving the mtime alone would
/// make the entry look overdue forever, so every later render would fire
/// another conditional request — the in-flight guard only collapses
/// *overlapping* checks, not sequential ones.
async fn mark_checked(img_dir: &Path, path: &str) {
    let file = img_dir.join(cache_file_name(path));
    let now = std::time::SystemTime::now();
    let _ = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .and_then(|f| f.set_modified(now))
    })
    .await;
}

/// Fetch `path` from the upstream server with the bearer token and cache the
/// bytes + content type + validator on disk. `None` when offline /
/// unauthorized / error, and — deliberately — when the server answers 304,
/// which means the caller's cached copy is already current.
///
/// Strictly 200-only otherwise: `/api/thumbs/*` answers 202 while a
/// thumbnail is still being generated, and caching that placeholder would
/// wedge the thumb offline forever.
async fn fetch_and_cache(img_dir: &Path, path: &str, if_none_match: Option<&str>) -> Fetched {
    let Some(base) = upstream_url() else {
        return Fetched::Failed;
    };
    let url = format!("{base}{path}");
    let mut req = crate::data::with_bearer(crate::data::http_client().get(&url));
    if let Some(etag) = if_none_match {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(etag) {
            req = req.header(reqwest::header::IF_NONE_MATCH, value);
        }
    }
    let Ok(resp) = req.send().await else {
        return Fetched::Failed;
    };
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        // The copy on disk is current. Restart its window so the next
        // render doesn't ask again.
        mark_checked(img_dir, path).await;
        return Fetched::Unchanged;
    }
    if resp.status() != reqwest::StatusCode::OK {
        return Fetched::Failed;
    }
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let etag = resp
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let Ok(body) = resp.bytes().await else {
        return Fetched::Failed;
    };
    let bytes = body.to_vec();
    let name = cache_file_name(path);
    // Best-effort cache write, and async throughout: this runs on the
    // loopback's single-threaded runtime — from a detached task, now that
    // revalidation is a thing — so a blocking write here stalls the
    // `/dl/*` Range reads feeding playback.
    // Unique per write: a shared `{name}.tmp` is a corruption path the
    // moment two writers overlap, and defence in depth is cheap here.
    static WRITE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = WRITE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = img_dir.join(format!("{name}.{seq}.tmp"));
    if tokio::fs::write(&tmp, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, img_dir.join(&name)).await;
        let _ = tokio::fs::write(img_dir.join(format!("{name}.ct")), &ct).await;
        let etag_path = img_dir.join(format!("{name}.etag"));
        match &etag {
            Some(etag) => {
                let _ = tokio::fs::write(&etag_path, etag).await;
            }
            // A server that stopped publishing a validator must not leave
            // the previous one behind to be replayed against new bytes.
            None => {
                let _ = tokio::fs::remove_file(&etag_path).await;
            }
        }
    }
    Fetched::Replaced {
        bytes,
        content_type: ct,
    }
}

/// Oldest-first prune of the image cache down to `cap` bytes.
fn prune_img_cache(img_dir: &Path, cap: u64) {
    let Ok(dir) = std::fs::read_dir(img_dir) else {
        return;
    };

    // Group by cache key rather than by file. The bytes, the `.ct` and the
    // `.etag` are one logical entry, and their mtimes drift apart — a 304
    // touches only the bytes — so pruning them independently deletes an
    // entry's validator while keeping its image. That image then has
    // nothing to revalidate with and is skipped forever, which is exactly
    // the stale cover this cache exists to prevent.
    let mut entries: HashMap<String, CacheEntryFiles> = HashMap::new();
    for file in dir.flatten() {
        let Ok(meta) = file.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let path = file.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // `{key}`, `{key}.ct`, `{key}.etag`, and any `{key}.tmp` left by an
        // interrupted write all belong to `{key}`.
        let key = name
            .rsplit_once('.')
            .filter(|(_, ext)| matches!(*ext, "ct" | "etag" | "tmp"))
            .map(|(stem, _)| stem)
            .unwrap_or(name);
        let entry = entries.entry(key.to_string()).or_default();
        entry.bytes += meta.len();
        entry.paths.push(path);
        // Freshest wins: the bytes carry the last-checked time, so an entry
        // confirmed current by a 304 is treated as recently used.
        if let Ok(modified) = meta.modified() {
            entry.modified = entry.modified.max(Some(modified));
        }
    }

    let total: u64 = entries.values().map(|e| e.bytes).sum();
    if total <= cap {
        return;
    }
    let mut ordered: Vec<CacheEntryFiles> = entries.into_values().collect();
    ordered.sort_by_key(|e| e.modified);

    let mut excess = total - cap;
    for entry in ordered {
        if excess == 0 {
            break;
        }
        // All of an entry's files go together, so a survivor is never left
        // without its validator.
        for path in &entry.paths {
            let _ = std::fs::remove_file(path);
        }
        excess = excess.saturating_sub(entry.bytes);
    }
}

/// Every file making up one cached image, so pruning treats them as a unit.
#[derive(Default)]
struct CacheEntryFiles {
    paths: Vec<PathBuf>,
    bytes: u64,
    modified: Option<std::time::SystemTime>,
}

/// Path-segment allowlist for `/dl/{uuid}/{file}`: uuids and our own file
/// names only, so no traversal escapes the downloads root.
fn sanitize_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Media paths the image proxy will fetch/cache — image reads only, never a
/// data or mutation endpoint.
fn proxy_path_allowed(path: &str) -> bool {
    if path.contains("..") || path.contains('?') {
        return false;
    }
    path.starts_with("/api/covers/")
        || path.starts_with("/api/thumbs/")
        || (path.starts_with("/api/authors/") && path.ends_with("/photo"))
        || path.starts_with("/api/journals/images/")
        || (path.starts_with("/api/suggestions/") && path.ends_with("/cover"))
}

/// Disk-cache file name for a proxied path (URL-safe base64, no padding).
fn cache_file_name(path: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path.as_bytes())
}

/// Content type for a downloaded file, from its extension. The inverse of
/// `downloads::engine::mime_ext` — keep the two maps in sync.
pub(crate) fn ext_mime(file: &str) -> &'static str {
    let ext = file.rsplit('.').next().unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "epub" => "application/epub+zip",
        "m4b" | "m4a" | "mp4" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "aac" => "audio/aac",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Minimal percent-encoding for a path riding inside a query value.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests;
