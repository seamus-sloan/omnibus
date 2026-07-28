//! Loopback media server for offline playback: a tiny axum server on
//! `127.0.0.1:{random port}` that serves downloaded book files (with Range
//! support, so `<audio>` can seek) and an offline image cache that proxies
//! covers/thumbs from the real server while online. Guarded by a per-boot
//! random `?token=` — the same query-token pattern the real server's media
//! routes use, since WebView subresource fetches can't carry a bearer
//! header. Every response carries `Access-Control-Allow-Origin: *` because
//! the WebView origin is wry's custom scheme (`dioxus://` on iOS), exactly
//! mirroring the server's own `/api/ebooks/{uuid}/file` CORS behavior.

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

use self::range::RangeOutcome;

pub mod range;

/// Where the running loopback server can be reached this boot.
pub struct LoopbackInfo {
    pub port: u16,
    pub token: String,
}

/// Soft cap for the proxied-image disk cache; pruned oldest-first at boot.
const IMG_CACHE_CAP_BYTES: u64 = 256 * 1024 * 1024;

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
    if cached_image(&img_dir, path).is_some() {
        return;
    }
    let _ = fetch_and_cache(&img_dir, path).await;
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

async fn img_handler(State(st): State<Arc<MediaState>>, Query(q): Query<ImgQuery>) -> Response {
    if q.token.as_deref() != Some(st.token.as_str()) {
        return plain(StatusCode::FORBIDDEN, "bad token");
    }
    let Some(path) = q.path.filter(|p| proxy_path_allowed(p)) else {
        return plain(StatusCode::NOT_FOUND, "not found");
    };
    let bust = q.v.unwrap_or(0) > 0;
    if !bust {
        if let Some((bytes, ct)) = cached_image(&st.img_dir, &path) {
            // A cover replaced from another device changes these bytes under
            // an unchanged URL, so a cache hit is not on its own an answer.
            return match refresh_if_superseded(&st.img_dir, &path).await {
                Some((fresh, fresh_ct)) => img_response(fresh, &fresh_ct),
                None => img_response(bytes, &ct),
            };
        }
    }
    match fetch_and_cache(&st.img_dir, &path).await {
        Some((bytes, ct)) => img_response(bytes, &ct),
        // A bust-requested refetch that fails offline still prefers the
        // stale cached copy over a broken image; an uncached thumb falls
        // back to the cached full cover (thumbs 202 until generated, so a
        // download may have only warmed the cover).
        None => match cached_image(&st.img_dir, &path)
            .or_else(|| thumb_cover_fallback(&path).and_then(|p| cached_image(&st.img_dir, &p)))
        {
            Some((bytes, ct)) => img_response(bytes, &ct),
            None => plain(StatusCode::NOT_FOUND, "not cached"),
        },
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

fn img_response(bytes: Vec<u8>, content_type: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response"))
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Body::from(msg))
        .unwrap_or_default()
}

/// Read a cached image + its recorded content type.
fn cached_image(img_dir: &Path, path: &str) -> Option<(Vec<u8>, String)> {
    let name = cache_file_name(path);
    let bytes = std::fs::read(img_dir.join(&name)).ok()?;
    let ct = std::fs::read_to_string(img_dir.join(format!("{name}.ct")))
        .unwrap_or_else(|_| "image/jpeg".into());
    Some((bytes, ct))
}

/// Outcome of a conditional fetch through the image proxy.
enum ImageFetch {
    /// New bytes, already written to the cache.
    Fresh(Vec<u8>, String),
    /// The server confirmed the cached copy is still current.
    Unchanged,
    /// Offline, unauthorized, still generating, or a transport error — every
    /// one of which means "keep serving whatever is on disk".
    Failed,
}

/// Fetch `path` from the upstream server with the bearer token and cache the
/// bytes, content type, and validator on disk.
///
/// Strictly 200-only for caching: `/api/thumbs/*` answers 202 while a
/// thumbnail is still being generated, and storing that placeholder would
/// wedge the thumb offline forever. Passing `if_none_match` turns this into a
/// revalidation, where a 304 is the cheap answer that nothing changed.
async fn fetch_image(img_dir: &Path, path: &str, if_none_match: Option<&str>) -> ImageFetch {
    let Some(base) = upstream_url() else {
        return ImageFetch::Failed;
    };
    let url = format!("{base}{path}");
    let mut req = crate::data::with_bearer(crate::data::http_client().get(&url));
    if let Some(etag) = if_none_match {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(etag) {
            req = req.header(reqwest::header::IF_NONE_MATCH, value);
        }
    }
    let Ok(resp) = req.send().await else {
        return ImageFetch::Failed;
    };
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return ImageFetch::Unchanged;
    }
    if resp.status() != reqwest::StatusCode::OK {
        return ImageFetch::Failed;
    }
    let header_str = |name: reqwest::header::HeaderName| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let ct = header_str(header::CONTENT_TYPE).unwrap_or_else(|| "image/jpeg".into());
    let etag = header_str(reqwest::header::ETAG);
    let Ok(body) = resp.bytes().await else {
        return ImageFetch::Failed;
    };
    let bytes = body.to_vec();
    let name = cache_file_name(path);
    // Best-effort cache write; serving the fetched bytes matters more.
    let tmp = img_dir.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, img_dir.join(&name));
        let _ = std::fs::write(img_dir.join(format!("{name}.ct")), &ct);
        match &etag {
            Some(etag) => {
                let _ = std::fs::write(img_dir.join(format!("{name}.etag")), etag);
            }
            // A server too old to send one must not leave a stale tag behind
            // that later revalidations would quote back at it.
            None => {
                let _ = std::fs::remove_file(img_dir.join(format!("{name}.etag")));
            }
        }
    }
    ImageFetch::Fresh(bytes, ct)
}

/// Fetch and cache `path` unconditionally.
async fn fetch_and_cache(img_dir: &Path, path: &str) -> Option<(Vec<u8>, String)> {
    match fetch_image(img_dir, path, None).await {
        ImageFetch::Fresh(bytes, ct) => Some((bytes, ct)),
        ImageFetch::Unchanged | ImageFetch::Failed => None,
    }
}

/// Paths already revalidated against the server this process run.
///
/// The bound that makes revalidation affordable: covers are requested once
/// per grid cell per scroll, and a conditional request on every one of those
/// would put dozens of round-trips behind a flick. Checking each distinct
/// image once per run still picks up a cover changed on another device — the
/// next launch, or the next time that image is first drawn — without making
/// scrolling pay for it.
fn revalidated() -> &'static RwLock<std::collections::HashSet<String>> {
    static SEEN: OnceLock<RwLock<std::collections::HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| RwLock::new(std::collections::HashSet::new()))
}

/// The validator the cached copy of `path` was stored under.
fn cached_etag(img_dir: &Path, path: &str) -> Option<String> {
    let name = cache_file_name(path);
    let etag = std::fs::read_to_string(img_dir.join(format!("{name}.etag"))).ok()?;
    (!etag.trim().is_empty()).then_some(etag)
}

/// Ask the server whether the cached copy of `path` is still the one it
/// holds, at most once per path per process run.
///
/// `Some` when it was not and new bytes are now cached; `None` when it was
/// current, when the check was skipped, or when the server could not be
/// reached — all of which mean "serve what is on disk". An entry cached
/// before validators were recorded has nothing to ask with, so it refetches
/// once and is conditional from then on.
async fn refresh_if_superseded(img_dir: &Path, path: &str) -> Option<(Vec<u8>, String)> {
    if super::sync::is_offline() {
        return None;
    }
    if revalidated().read().is_ok_and(|seen| seen.contains(path)) {
        return None;
    }
    let outcome = fetch_image(img_dir, path, cached_etag(img_dir, path).as_deref()).await;
    // Only a definitive answer counts as checked; a failed reach should be
    // retried once the connection is back, not written off for the run.
    if !matches!(outcome, ImageFetch::Failed) {
        if let Ok(mut seen) = revalidated().write() {
            seen.insert(path.to_string());
        }
    }
    match outcome {
        ImageFetch::Fresh(bytes, ct) => Some((bytes, ct)),
        ImageFetch::Unchanged | ImageFetch::Failed => None,
    }
}

/// Oldest-first prune of the image cache down to `cap` bytes.
fn prune_img_cache(img_dir: &Path, cap: u64) {
    let Ok(entries) = std::fs::read_dir(img_dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta.modified().ok()?;
            Some((e.path(), meta.len(), modified))
        })
        .collect();
    let total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if total <= cap {
        return;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut excess = total - cap;
    for (path, len, _) in files {
        if excess == 0 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            excess = excess.saturating_sub(len);
        }
    }
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
