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
