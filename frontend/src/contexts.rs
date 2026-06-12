//! App-wide Dioxus contexts and their typed accessors.
//!
//! Holds the small set of values every page reads: the API base URL (mobile
//! injects, web is relative) and the cross-route search-query signal owned
//! by [`crate::App`] and consumed by the nav and landing page.

use dioxus::prelude::*;

#[cfg(feature = "mobile")]
use crate::data;

/// Return the base URL for API calls. Mobile reads it from the `ServerUrl`
/// context; web co-locates with the server so the base is empty/relative.
pub fn use_server_url() -> String {
    #[cfg(feature = "mobile")]
    {
        use_context::<data::ServerUrl>().0
    }
    #[cfg(not(feature = "mobile"))]
    {
        String::new()
    }
}

/// Cross-route search query. Owned by [`App`] via `use_context_provider`
/// so the [`Nav`]-hosted search box and the [`LandingPage`] read/write the
/// same signal — typing in the nav from any route updates the landing
/// results without a route-param round-trip.
#[derive(Copy, Clone)]
pub struct SearchQuery(pub Signal<String>);

/// Convenience accessor for the search-query context.
pub fn use_search_query() -> SearchQuery {
    use_context::<SearchQuery>()
}

/// App-wide cached `/api/auth/me` result. Owned by [`App`] via
/// `use_context_provider` so every component that needs to gate on
/// `is_admin` (top nav avatar, landing inline edits, author Delete) reads
/// from one signal instead of each firing its own `current_user()` round
/// trip on mount. Without this every navigation would re-fetch `/me`
/// from N components and race the `/api/auth/*` rate-limit budget.
///
/// The outer `Option` is "not yet resolved since boot" (pre-hydration on
/// SSR, or the very first tick before the boot effect lands). The inner
/// `Option` is "resolved": `None` means an explicit 401 (unauthenticated),
/// `Some(u)` means authenticated. Transient errors (network blip, 429)
/// leave the outer `None` in place so the UI keeps showing the
/// placeholder rather than briefly swapping to "Log in".
///
/// Web-only: mobile uses bearer tokens via `token_store` and never hits
/// `/api/auth/me`. The context isn't provided on mobile and
/// [`use_current_user`] is not callable there.
#[cfg(not(feature = "mobile"))]
#[derive(Copy, Clone)]
pub struct CurrentUser(pub Signal<Option<Option<omnibus_shared::UserSummary>>>);

/// Convenience accessor for the cached-user context. Web/SSR only.
#[cfg(not(feature = "mobile"))]
pub fn use_current_user() -> CurrentUser {
    use_context::<CurrentUser>()
}

/// App-wide audiobook playback state. Owned by [`crate::App`] via
/// `use_context_provider` so the full listen player and the persistent
/// mini-dock share one set of signals. Playback survives route changes
/// because both these signals and the backing `<audio>` element live at the
/// App root, not inside the route component.
///
/// `uuid` is the currently-loaded audiobook (`None` = nothing playing).
/// Setting it (the listen page on mount) or clearing it (the dock's dismiss
/// button) drives the App-level bootstrap effect to load / swap / tear down
/// playback. Provided unconditionally on `not(mobile)` so SSR markup matches
/// the WASM client; mobile stubs the listen page and never provides it.
#[cfg(not(feature = "mobile"))]
#[derive(Copy, Clone)]
pub struct PlaybackState {
    pub uuid: Signal<Option<String>>,
    pub book: Signal<Option<omnibus_shared::EbookMetadata>>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    pub duration: Signal<f64>,
    pub elapsed: Signal<f64>,
    pub playing: Signal<bool>,
    pub rate: Signal<f64>,
    pub chapters: Signal<Vec<omnibus_shared::ChapterInfo>>,
    pub hls_ready: Signal<bool>,
    pub playback_failed: Signal<bool>,
}

#[cfg(not(feature = "mobile"))]
impl PlaybackState {
    /// Construct the initial (nothing-playing) playback state.
    pub fn new() -> Self {
        Self {
            uuid: Signal::new(None),
            book: Signal::new(None),
            loading: Signal::new(false),
            error: Signal::new(None),
            duration: Signal::new(0.0),
            elapsed: Signal::new(0.0),
            playing: Signal::new(false),
            rate: Signal::new(1.0),
            chapters: Signal::new(Vec::new()),
            hls_ready: Signal::new(false),
            playback_failed: Signal::new(false),
        }
    }
}

#[cfg(not(feature = "mobile"))]
impl Default for PlaybackState {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience accessor for the playback context. Web/SSR only.
#[cfg(not(feature = "mobile"))]
pub fn use_playback() -> PlaybackState {
    use_context::<PlaybackState>()
}
