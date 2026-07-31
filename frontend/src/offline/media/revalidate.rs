//! Background ETag revalidation of the proxied-image cache: a cached image
//! is served immediately, then checked against the server *after* the
//! response has gone out, so revalidation never adds latency. [`Fetched`]
//! is one upstream fetch's outcome; the claim/lock machinery caps a grid
//! scroll to one check per stale image and stops it racing a `?v=` bust.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::CachedImage;

/// What one upstream fetch produced.
pub(super) enum Fetched {
    /// New bytes, already written to the cache.
    Replaced {
        bytes: Vec<u8>,
        content_type: String,
    },
    /// The server confirmed the cached copy is current (304).
    Unchanged,
    /// Offline, unauthorized, or an answer we refuse to cache — notably the
    /// 202 `/api/thumbs/*` returns while a thumbnail is still generating,
    /// which cached would wedge the thumb offline forever.
    Failed,
}

/// Ask the server whether a cached image has changed, and replace it when it
/// has. Runs *after* the cached copy has already been served, so it never
/// adds latency to a cover render — the newer image lands on the next one.
///
/// Skipped while offline, while the cached copy is younger than
/// [`super::IMG_REVALIDATE_AFTER_SECS`], and while another revalidation of
/// the same path is already in flight — a grid scroll must not turn into
/// one request per visible cover, repeatedly.
pub(super) fn revalidate_in_background(img_dir: PathBuf, path: String, cached: &CachedImage) {
    // Cheap pre-check so an in-window render doesn't spawn a task at all.
    // It is *not* the decision — that is re-taken under the write lock
    // below, because by the time this task runs the entry may have been
    // replaced by an explicit `?v=` bust.
    if cached.age_secs < super::IMG_REVALIDATE_AFTER_SECS
        || cached.etag.is_none()
        || crate::offline::sync::is_offline()
    {
        return;
    }
    if !claim_revalidation(&path) {
        return;
    }
    tokio::spawn(async move {
        let _guard = lock_cache_key(&path).await;
        // Re-read under the lock. A bust that landed while this task was
        // queued has already written newer bytes and restarted the window;
        // this request's answer describes the cover as it was *before* that
        // edit, so applying it would overwrite the new cover with the old.
        match super::cached_image(&img_dir, &path).await {
            Some(current) if current.age_secs >= super::IMG_REVALIDATE_AFTER_SECS => {
                if let Some(etag) = current.etag.as_deref() {
                    let _ = super::fetch_and_cache(&img_dir, &path, Some(etag)).await;
                }
            }
            _ => {}
        }
        drop(_guard);
        release_revalidation(&path);
    });
}

/// Serializes every write to one cache key.
///
/// Two writers for the same image are not merely wasteful, they corrupt:
/// the foreground `?v=` bust and a background revalidation used to write
/// the same temp file concurrently and then each rename it into place.
/// Holding this across fetch-and-write also gives the background task a
/// point at which to notice that a bust replaced the entry while it was
/// queued — see [`revalidate_in_background`].
pub(super) async fn lock_cache_key(path: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut map = write_locks().lock().unwrap_or_else(|e| e.into_inner());
        // Opportunistic sweep: a lock nobody else holds is one nobody is
        // waiting on, so the map stays proportional to live writes rather
        // than to every image ever served.
        if map.len() > 256 {
            map.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        map.entry(path.to_string()).or_default().clone()
    };
    lock.lock_owned().await
}

fn write_locks() -> &'static std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

/// Paths with a revalidation currently in flight.
fn revalidating() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static IN_FLIGHT: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        OnceLock::new();
    IN_FLIGHT.get_or_init(Default::default)
}

/// `true` when this caller took ownership of revalidating `path`.
fn claim_revalidation(path: &str) -> bool {
    revalidating()
        .lock()
        .map(|mut set| set.insert(path.to_string()))
        .unwrap_or(false)
}

fn release_revalidation(path: &str) {
    if let Ok(mut set) = revalidating().lock() {
        set.remove(path);
    }
}
