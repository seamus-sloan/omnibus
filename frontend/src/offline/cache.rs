//! Typed access to the replica cache plus the cache-first read policy.
//!
//! `read_through` is the single read policy for the mobile data layer:
//! serve the cached copy immediately (local-first — offline or slow
//! networks never block a paint), and when the row is stale, revalidate
//! against the server in the background, bumping [`subscribe`]'s
//! generation channel when the payload actually changed so rendered pages
//! re-read. A cache miss falls back to a foreground fetch — or fails fast
//! with [`DataError::Offline`] while known-offline. Server answers are
//! never masked: HTTP errors, 401s, and decode failures on the foreground
//! path propagate untouched, and a failed revalidation writes nothing.
//! `read_through_network_first` remains for the few reads where acting on
//! a stale answer would be a correctness bug (identity, progress LWW).

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::watch;

use crate::data::DataError;

use super::{media, store, sync};

/// Cache keys for every replicated endpoint. One builder per entity so the
/// readers, the optimistic-apply code, and the download engine can never
/// drift on formatting.
pub mod keys {
    /// `GET /api/auth/me`.
    pub fn me() -> String {
        "me".into()
    }
    /// `GET /api/library`.
    pub fn library() -> String {
        "library".into()
    }
    /// First page of `GET /api/ebooks` (landing's initial fetch).
    pub fn ebooks_first(sort: &str, dir: &str, formats: &str) -> String {
        format!("ebooks_first:{sort}:{dir}:{formats}")
    }
    /// The full-library replica (every row, background-synced).
    pub fn ebooks_all() -> String {
        "ebooks_all".into()
    }
    /// `GET /api/ebooks/{uuid}`.
    pub fn ebook(uuid: &str) -> String {
        format!("ebook:{uuid}")
    }
    /// `GET /api/audiobooks/{uuid}/manifest?file_id=`.
    pub fn manifest(uuid: &str, file_id: Option<i64>) -> String {
        format!("manifest:{uuid}:{}", file_id.unwrap_or(-1))
    }
    /// `GET /api/progress/{uuid}?format=`.
    pub fn progress(uuid: &str, format: &str) -> String {
        format!("progress:{uuid}:{format}")
    }
    /// `GET /api/progress/recent?limit=`.
    pub fn recent_progress(limit: i64) -> String {
        format!("recent_progress:{limit}")
    }
    /// `GET /api/audiobooks/{uuid}/playback-rate`.
    pub fn playback_rate(uuid: &str) -> String {
        format!("rate:{uuid}")
    }
    /// `GET /api/highlights/book/{uuid}`.
    pub fn highlights(uuid: &str) -> String {
        format!("highlights:{uuid}")
    }
    /// `GET /api/bookmarks/book/{uuid}`.
    pub fn bookmarks(uuid: &str) -> String {
        format!("bookmarks:{uuid}")
    }
    /// `GET /api/journals/book/{uuid}`.
    pub fn journals(uuid: &str) -> String {
        format!("journals:{uuid}")
    }
    /// `GET /api/ratings/{uuid}`.
    pub fn rating(uuid: &str) -> String {
        format!("rating:{uuid}")
    }
    /// `GET /api/ratings/others/{uuid}`.
    pub fn ratings_others(uuid: &str) -> String {
        format!("ratings_others:{uuid}")
    }
    /// `GET /api/shelves`.
    pub fn shelves() -> String {
        "shelves".into()
    }
    /// `GET /api/shelves/{id}`.
    pub fn shelf(id: i64) -> String {
        format!("shelf:{id}")
    }
    /// `GET /api/shelves/{id}/page?sort=&dir=`.
    pub fn shelf_page(id: i64, sort: &str, dir: &str) -> String {
        format!("shelf_page:{id}:{sort}:{dir}")
    }
    /// `GET /api/authors`.
    pub fn authors() -> String {
        "authors".into()
    }
    /// `GET /api/authors/{id}`.
    pub fn author(id: i64) -> String {
        format!("author:{id}")
    }
    /// `GET /api/series`.
    pub fn series_index() -> String {
        "series".into()
    }
    /// `GET /api/series/{id}`.
    pub fn series(id: i64) -> String {
        format!("series:{id}")
    }
    /// `GET /api/tags`.
    pub fn tags() -> String {
        "tags".into()
    }
    /// `GET /api/genres`.
    pub fn genres() -> String {
        "genres".into()
    }
    /// `GET /api/stats?range=`.
    pub fn stats(range: &str) -> String {
        format!("stats:{range}")
    }
    /// `GET /api/ebooks/{uuid}/suggestions`.
    pub fn suggestions(uuid: &str) -> String {
        format!("suggestions:{uuid}")
    }
    /// Durable mirror of the in-memory reader position map.
    pub fn reader_cfi(uuid: &str) -> String {
        format!("reader_cfi:{uuid}")
    }
    /// Durable mirror of the in-memory audio position map.
    pub fn audio_pos(uuid: &str) -> String {
        format!("audio_pos:{uuid}")
    }
    /// Durable mirror of the per-user audio playback-rate map.
    pub fn audio_rate(user_id: i64, uuid: &str) -> String {
        format!("audio_rate:{user_id}:{uuid}")
    }
}

/// Read one cached value, deserialized. `None` on miss, decode failure, or
/// no store.
pub async fn get_json<T: DeserializeOwned>(key: &str) -> Option<T> {
    let row = store::store()?.kv_get(key).await?;
    match serde_json::from_str(&row.payload) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, key, "offline cache payload failed to decode; dropping");
            store::store()?.kv_delete(key);
            None
        }
    }
}

/// Serialize and store one cached value (best-effort, fire-and-forget).
pub fn put_json<T: Serialize>(key: &str, value: &T) {
    let Some(store) = store::store() else { return };
    match serde_json::to_string(value) {
        Ok(payload) => store.kv_put(key, payload),
        Err(e) => tracing::warn!(error = %e, key, "offline cache payload failed to encode"),
    }
}

/// Patch a cached value in place (optimistic apply of a queued mutation).
/// A miss is a no-op — the entity list was never viewed, so there's nothing
/// to patch; the next online fetch supersedes anyway.
pub async fn mutate_json<T, F>(key: &str, f: F)
where
    T: DeserializeOwned + Serialize,
    F: FnOnce(&mut T),
{
    let Some(mut value) = get_json::<T>(key).await else {
        return;
    };
    f(&mut value);
    put_json(key, &value);
}

/// Serve-from-cache freshness window: a row younger than this is returned
/// without spawning a revalidation — which is also what terminates the
/// revalidate → generation-bump → page-refetch cycle.
const FRESH_WINDOW_SECS: i64 = 30;

/// Generation counter bumped whenever a background revalidation (or the
/// outbox drain) lands changed data; pages subscribe via
/// `use_cache_generation` and re-run their fetch effects, which are then
/// served from the fresh cache with zero network.
fn channel() -> &'static (watch::Sender<u64>, watch::Receiver<u64>) {
    static CH: OnceLock<(watch::Sender<u64>, watch::Receiver<u64>)> = OnceLock::new();
    CH.get_or_init(|| watch::channel(0))
}

/// Subscribe to cache-change generations.
pub fn subscribe() -> watch::Receiver<u64> {
    channel().0.subscribe()
}

/// Bump the generation channel (revalidation landed changed data, or the
/// drain rewrote cache rows).
pub(crate) fn notify_changed() {
    channel().0.send_modify(|g| *g += 1);
}

/// Force an immediate, awaitable server round-trip for `key` (the
/// pull-to-refresh path): bypasses the freshness window and lands the
/// result through the same compare-put-notify flow as a background
/// revalidation. No-ops when offline, when queued mutations are pending
/// (same guard as [`spawn_revalidation`]), or when a revalidation for the
/// key is already in flight.
pub async fn refresh<T, Fut>(key: String, fetch: Fut)
where
    T: Serialize + DeserializeOwned + Send + 'static,
    Fut: std::future::Future<Output = Result<T, DataError>> + Send + 'static,
{
    if sync::is_offline() || sync::state().pending_ops > 0 {
        return;
    }
    {
        let Ok(mut in_flight) = revalidating().lock() else {
            return;
        };
        if !in_flight.insert(key.clone()) {
            return;
        }
    }
    let prior_payload = get_row::<T>(&key)
        .await
        .map(|(_, _, payload)| payload)
        .unwrap_or_default();
    revalidate::<T, Fut>(key, prior_payload, fetch).await;
}

/// Keys with a revalidation currently in flight — one per key at a time.
fn revalidating() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// One cached row, decoded, plus its age and raw payload. Mirrors
/// [`get_json`]'s handling of undecodable rows (warn + drop).
async fn get_row<T: DeserializeOwned>(key: &str) -> Option<(T, i64, String)> {
    let row = store::store()?.kv_get(key).await?;
    match serde_json::from_str(&row.payload) {
        Ok(v) => Some((v, row.fetched_at, row.payload)),
        Err(e) => {
            tracing::warn!(error = %e, key, "offline cache payload failed to decode; dropping");
            store::store()?.kv_delete(key);
            None
        }
    }
}

/// Cache-first read (stale-while-revalidate; see module docs). The cache
/// key must be a `keys::*` builder output.
pub async fn read_through<T, Fut>(key: String, fetch: Fut) -> Result<T, DataError>
where
    T: Serialize + DeserializeOwned + Send + 'static,
    Fut: std::future::Future<Output = Result<T, DataError>> + Send + 'static,
{
    match get_row::<T>(&key).await {
        Some((value, fetched_at, payload)) => {
            if store::now_secs() - fetched_at >= FRESH_WINDOW_SECS {
                spawn_revalidation(key, payload, fetch);
            }
            Ok(value)
        }
        None if sync::is_offline() => Err(DataError::Offline),
        None => match fetch.await {
            Ok(value) => {
                sync::note_online();
                put_json(&key, &value);
                Ok(value)
            }
            Err(e) if sync::is_offline_error(&e) => {
                sync::note_offline();
                Err(e)
            }
            Err(e) => {
                // The server answered (4xx/5xx/decode) — we're online;
                // don't mask its answer with stale data.
                sync::note_online();
                Err(e)
            }
        },
    }
}

/// Network-first read with offline cache fallback, for flows that must not
/// act on a stale answer (identity for the account-switch wipe, progress
/// for the cross-device LWW reconcile). While known-offline it serves the
/// cache immediately — or fails fast — instead of waiting out a doomed
/// connect.
pub async fn read_through_network_first<T, Fut>(key: String, fetch: Fut) -> Result<T, DataError>
where
    T: Serialize + DeserializeOwned,
    Fut: std::future::Future<Output = Result<T, DataError>>,
{
    if sync::is_offline() {
        return match get_json::<T>(&key).await {
            Some(cached) => Ok(cached),
            None => Err(DataError::Offline),
        };
    }
    match fetch.await {
        Ok(value) => {
            sync::note_online();
            put_json(&key, &value);
            Ok(value)
        }
        Err(e) if sync::is_offline_error(&e) => {
            sync::note_offline();
            match get_json::<T>(&key).await {
                Some(cached) => Ok(cached),
                None => Err(e),
            }
        }
        Err(e) => {
            sync::note_online();
            Err(e)
        }
    }
}

/// Kick a background revalidation of `key`. Skipped while offline (the
/// fetch is doomed; the reconnect path re-triggers reads) or while outbox
/// ops are pending (a server read would clobber optimistic patches — the
/// drain-completion bump re-triggers reads once the server has them).
fn spawn_revalidation<T, Fut>(key: String, prior_payload: String, fetch: Fut)
where
    T: Serialize + DeserializeOwned + Send + 'static,
    Fut: std::future::Future<Output = Result<T, DataError>> + Send + 'static,
{
    if sync::is_offline() || sync::state().pending_ops > 0 {
        return;
    }
    {
        let Ok(mut in_flight) = revalidating().lock() else {
            return;
        };
        if !in_flight.insert(key.clone()) {
            return;
        }
    }
    let task_key = key.clone();
    let spawned = spawn_task(async move {
        revalidate::<T, Fut>(key, prior_payload, fetch).await;
    });
    if !spawned {
        if let Ok(mut in_flight) = revalidating().lock() {
            in_flight.remove(&task_key);
        }
    }
}

async fn revalidate<T, Fut>(key: String, prior_payload: String, fetch: Fut)
where
    T: Serialize,
    Fut: std::future::Future<Output = Result<T, DataError>>,
{
    match fetch.await {
        Ok(value) => {
            sync::note_online();
            if let (Some(st), Ok(payload)) = (store::store(), serde_json::to_string(&value)) {
                let changed = payload != prior_payload;
                // Re-stamp fetched_at even when unchanged, so the fresh
                // window keeps unchanged keys quiet.
                st.kv_put(&key, payload);
                if changed {
                    notify_changed();
                }
            }
        }
        Err(e) if sync::is_offline_error(&e) => sync::note_offline(),
        Err(e) => {
            // Server answered but rejected/garbled — keep the stale-but-
            // valid render, write nothing. A 401 already cleared the token
            // inside the `_online` fetcher (note_status), so the login
            // redirect still fires.
            sync::note_online();
            tracing::debug!(error = %e, key, "background revalidation failed; keeping cached copy");
        }
    }
    if let Ok(mut in_flight) = revalidating().lock() {
        in_flight.remove(&key);
    }
}

/// Spawn a revalidation: the loopback runtime when up (it outlives Dioxus
/// scopes, like downloads), else the ambient tokio runtime (tests, and
/// reads racing ahead of `media::start` at boot).
fn spawn_task<F>(fut: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if media::runtime_available() {
        return media::spawn_on_runtime(fut);
    }
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(fut);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
