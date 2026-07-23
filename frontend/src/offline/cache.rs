//! Typed access to the replica cache plus the network-first read policy.
//!
//! `read_through` is the single read policy for the mobile data layer:
//! always try the server first (fresh data wins — a just-uploaded book must
//! appear), refresh the cache on success, and serve the cached copy only
//! when the failure was connectivity-class. HTTP errors, 401s, and decode
//! failures propagate untouched — the server was reachable, so masking its
//! answer with stale data would be wrong.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::data::DataError;

use super::{store, sync};

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
    /// `GET /api/progress/recent`.
    pub fn recent_progress() -> String {
        "recent_progress".into()
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

/// Network-first read with offline fallback (see module docs). The cache
/// key must be a `keys::*` builder output.
pub async fn read_through<T, Fut>(key: String, fetch: Fut) -> Result<T, DataError>
where
    T: Serialize + DeserializeOwned,
    Fut: std::future::Future<Output = Result<T, DataError>>,
{
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
            // The server answered (4xx/5xx/decode) — we're online; don't
            // mask its answer with stale data.
            sync::note_online();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests;
