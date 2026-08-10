//! Per-library view-preference persistence for [`ViewPrefs`], layered on the
//! durable [`crate::client_store`] (localStorage on web, a JSON file on mobile,
//! inert on SSR so first-hydration markup matches the WASM client). Keyed per
//! library path so each library keeps independent sort/filter state. Shape
//! lives in `omnibus-shared`.

use omnibus_shared::ViewPrefs;

const STORAGE_PREFIX: &str = "omnibus.view_prefs::";

/// Single library-wide pointer to the path [`save`] last wrote prefs for.
/// The landing page learns its own library path only from a page-1 *response*,
/// so without this pointer its first fetch is forced to go out on
/// [`ViewPrefs::default`] and the grid paints in the wrong order until a second
/// fetch corrects it (#1818). Reading the pointer on mount breaks that cycle.
const LAST_LIBRARY_KEY: &str = "omnibus.view_prefs.last_library";

fn storage_key(library_path: &str) -> String {
    format!("{STORAGE_PREFIX}{library_path}")
}

/// Load view preferences for `library_path`. Returns [`ViewPrefs::default`]
/// when no record exists, when storage is unavailable, or when the stored JSON
/// fails to parse.
pub fn load(library_path: &str) -> ViewPrefs {
    crate::client_store::get(&storage_key(library_path))
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist `prefs` for `library_path`. Serialization or storage failures
/// (private mode, quota, an unwritable data dir) are silently ignored — the UI
/// keeps its in-memory copy regardless.
///
/// Also records `library_path` as the last-browsed library so [`load_last`] can
/// find these prefs before a fetch has revealed which library is on screen. That
/// write is skipped when the pointer already names this path: the mobile
/// `client_store` reserializes and re-flushes its whole map on every `set`, so
/// re-stamping an unchanged path on each sort toggle is pure I/O.
///
/// Storage is best-effort and swallows failures, so the pointer can outlive a
/// prefs write that never landed. That degrades to [`load`] handing back
/// [`ViewPrefs::default`] for the pointed-at key — exactly what a missing
/// pointer would have produced — so a lost write costs the extra fetch this
/// pointer exists to avoid rather than applying someone else's prefs.
pub fn save(library_path: &str, prefs: &ViewPrefs) {
    if let Ok(raw) = serde_json::to_string(prefs) {
        crate::client_store::set(&storage_key(library_path), &raw);
        if crate::client_store::get(LAST_LIBRARY_KEY).as_deref() != Some(library_path) {
            crate::client_store::set(LAST_LIBRARY_KEY, library_path);
        }
    }
}

/// The library path [`save`] last persisted prefs for, or `None` on a
/// first-ever visit and under SSR (where `client_store` is inert).
pub fn last_library() -> Option<String> {
    crate::client_store::get(LAST_LIBRARY_KEY).filter(|path| !path.is_empty())
}

/// Prefs for the last-browsed library, for hydrating before the library path is
/// known. `None` — rather than [`ViewPrefs::default`] — when no pointer is
/// stored, so a caller can tell "nothing was ever saved" from "defaults were
/// saved" and skip a pointless signal write.
///
/// The result is a *guess*: the viewer may since have switched libraries. The
/// landing page still reconciles against the authoritative path once a fetch
/// returns it, so a stale pointer costs the one extra fetch this exists to
/// avoid rather than leaving the wrong prefs applied.
pub fn load_last() -> Option<ViewPrefs> {
    last_library().map(|path| load(&path))
}

// Mobile tests — the only non-wasm target where `client_store` actually
// persists, so it's the only place `load_last`'s positive path and `save`'s
// dedup-write skip can be exercised without a browser. The mobile backend is
// one process-global map flushed to a file under `$HOME` (`client_store.rs`),
// so tests serialize on `TEST_GUARD` and reset it between cases rather than
// touching a developer's real `$HOME/.omnibus`.
#[cfg(all(test, feature = "mobile"))]
mod mobile_tests {
    use super::*;
    use omnibus_shared::{SortDir, SortKey, ViewFilters, ViewMode};
    use std::sync::Mutex;

    static TEST_GUARD: Mutex<()> = Mutex::new(());

    /// Run `f` with `$HOME` redirected to a scratch dir and the mobile store
    /// reset, so the mobile backend's disk flush (`client_store.rs`) lands in
    /// a throwaway location instead of a developer's real `~/.omnibus`.
    /// Nothing else in this crate reads or writes `$HOME` under `test`, so
    /// this is safe alongside `TEST_GUARD` serializing the map/call-counter
    /// state that lives independently of the env var.
    fn with_isolated_store(f: impl FnOnce()) {
        let scratch = tempfile::tempdir().expect("tempdir for isolated client_store");
        let prior_home = std::env::var_os("HOME");
        std::env::set_var("HOME", scratch.path());
        crate::client_store::reset_for_test();

        f();

        crate::client_store::reset_for_test();
        match prior_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }

    fn sample_prefs() -> ViewPrefs {
        ViewPrefs {
            view_mode: ViewMode::Table,
            sort_key: SortKey::Author,
            sort_dir: SortDir::Desc,
            filters: ViewFilters {
                formats: vec!["epub".into()],
                ..Default::default()
            },
            filters_open: true,
        }
    }

    #[test]
    fn load_last_returns_the_saved_prefs_after_save_records_the_pointer() {
        let _guard = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        with_isolated_store(|| {
            assert_eq!(last_library(), None);
            assert_eq!(load_last(), None);

            let prefs = sample_prefs();
            save("/library/a", &prefs);

            assert_eq!(last_library(), Some("/library/a".to_string()));
            assert_eq!(load_last(), Some(prefs));
        });
    }

    #[test]
    fn save_skips_the_pointer_write_when_the_library_already_matches() {
        let _guard = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        with_isolated_store(|| {
            // First save for this library: the pointer is unset, so it names
            // "/library/a" and also writes the pointer — two underlying
            // `client_store::set` calls (the per-library record, then the
            // pointer).
            save("/library/a", &ViewPrefs::default());
            let calls_after_first = crate::client_store::set_call_count_for_test();
            assert_eq!(calls_after_first, 2);

            // Re-saving the same library — even with different prefs — must
            // skip the pointer write, since it already names "/library/a".
            // Only the per-library record write should land, so the call
            // count grows by one, not two.
            let updated = sample_prefs();
            save("/library/a", &updated);
            let calls_after_second = crate::client_store::set_call_count_for_test();
            assert_eq!(calls_after_second - calls_after_first, 1);

            assert_eq!(load("/library/a"), updated);
            assert_eq!(last_library(), Some("/library/a".to_string()));
        });
    }
}

// SSR / server-only tests — exercise the no-persistence path that compiles
// under the default `cargo test -p omnibus-frontend --features server`. The
// `web`/`mobile` persistence lives in `client_store` behind its own cfgs and is
// unreachable here, so these assertions pin the documented SSR contract: every
// load returns defaults and save is an inert no-op.
#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;
    use omnibus_shared::{SortDir, SortKey, ViewFilters, ViewMode};

    #[test]
    fn load_always_returns_defaults() {
        assert_eq!(load("/library/a"), ViewPrefs::default());
        assert_eq!(load(""), ViewPrefs::default());
    }

    #[test]
    fn save_is_a_noop_and_does_not_affect_subsequent_loads() {
        let prefs = ViewPrefs {
            view_mode: ViewMode::Table,
            sort_key: SortKey::Author,
            sort_dir: SortDir::Desc,
            filters: ViewFilters {
                formats: vec!["epub".into()],
                ..Default::default()
            },
            filters_open: true,
        };
        // Persisting must not change what a later load returns under SSR.
        save("/library/a", &prefs);
        assert_eq!(load("/library/a"), ViewPrefs::default());
    }

    #[test]
    fn last_library_and_load_last_are_none_under_ssr() {
        // The landing page hydrates from these before its first fetch. Under
        // SSR they must stay empty, or the server would render a grid sorted
        // differently from the first WASM paint (rule 07).
        save("/library/a", &ViewPrefs::default());
        assert_eq!(last_library(), None);
        assert_eq!(load_last(), None);
    }

    #[test]
    fn default_prefs_match_documented_shape() {
        // First-hydration markup depends on these defaults: Grid view, sorted
        // by Title ascending, no active filters, sidebar closed.
        let d = ViewPrefs::default();
        assert_eq!(d.view_mode, ViewMode::Grid);
        assert_eq!(d.sort_key, SortKey::Title);
        assert_eq!(d.sort_dir, SortDir::Asc);
        assert!(d.filters.is_empty());
        assert!(!d.filters_open);
    }
}
