//! In-memory cache for the parsed ebook library.
//!
//! Scanning an ebook library means walking the directory tree, opening every
//! `.epub` as a zip, and parsing its OPF — expensive and redundant on every
//! page load since the filesystem rarely changes between requests. We keep
//! the last result in memory keyed by the configured library path. The
//! settings handler invalidates the cache whenever the path changes (or the
//! user re-saves settings), so the cache can never outlive its source of
//! truth.
//!
//! No file-watcher yet: if the user adds/removes books on disk without
//! going through settings, the cache will stay stale until settings are
//! re-saved or the server restarts. That's acceptable for now — noted in
//! `ROADMAP.md`.

use std::sync::Arc;

use omnibus_shared::EbookLibrary;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct EbookCache {
    inner: Arc<RwLock<Option<Cached>>>,
}

#[derive(Clone)]
struct Cached {
    path: Option<String>,
    library: EbookLibrary,
}

impl EbookCache {
    /// Return the cached library if it was computed for `path`.
    pub async fn get(&self, path: Option<&str>) -> Option<EbookLibrary> {
        let guard = self.inner.read().await;
        guard
            .as_ref()
            .filter(|c| c.path.as_deref() == path)
            .map(|c| c.library.clone())
    }

    /// Store `library` as the authoritative result for `path`.
    pub async fn set(&self, path: Option<String>, library: EbookLibrary) {
        let mut guard = self.inner.write().await;
        *guard = Some(Cached { path, library });
    }

    /// Drop whatever is cached. Called from settings handlers.
    pub async fn clear(&self) {
        let mut guard = self.inner.write().await;
        *guard = None;
    }
}

/// Look up in the cache; on miss, run the (blocking) scan on the blocking
/// pool, populate the cache, and return the result. Shared helper so both
/// the REST route and the RPC server function behave identically.
pub async fn load_or_scan(cache: &EbookCache, path: Option<String>) -> EbookLibrary {
    if let Some(hit) = cache.get(path.as_deref()).await {
        return hit;
    }
    let scan_path = path.clone();
    let library =
        tokio::task::spawn_blocking(move || crate::ebook::scan_ebook_library(scan_path.as_deref()))
            .await
            .unwrap_or_else(|e| EbookLibrary {
                path: path.clone(),
                books: vec![],
                error: Some(format!("ebook scan task failed: {e}")),
            });
    cache.set(path, library.clone()).await;
    library
}
