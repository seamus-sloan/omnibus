//! App-wide Dioxus contexts and their typed accessors.
//!
//! Holds the small set of values every page reads: the API base URL (mobile
//! injects, web is relative) and the cross-route search-query signal owned
//! by [`crate::App`] and consumed by the nav and landing page.

use dioxus::prelude::*;

#[cfg(feature = "mobile")]
use crate::data;

/// Return the base URL for API calls. Mobile reads it from the reactive
/// `ServerUrl` context signal (subscribing the caller so a URL change
/// re-renders it) and returns an owned snapshot; web co-locates with the
/// server so the base is empty/relative.
pub fn use_server_url() -> String {
    #[cfg(feature = "mobile")]
    {
        use_context::<data::ServerUrl>().0()
    }
    #[cfg(not(feature = "mobile"))]
    {
        String::new()
    }
}

/// The raw backend-URL signal, for the pre-login screens that *write* it
/// (the Connect screen on success). Readers should use [`use_server_url`].
#[cfg(feature = "mobile")]
pub fn use_server_url_signal() -> Signal<String> {
    use_context::<data::ServerUrl>().0
}

/// Build the URL for a media API path (`/api/covers/…`, `/api/thumbs/…`,
/// `/api/authors/{id}/photo`) rendered into an `<img src>`.
///
/// Web/SSR: returns `path` unchanged — same-origin and cookie-authed, so
/// the browser attaches the session automatically. Keeping it relative also
/// preserves SSR/WASM hydration parity.
///
/// Mobile: the WebView fetches `<img src>` itself, bypassing the native
/// `reqwest` client that carries the bearer token — and there's no session
/// cookie, since mobile auth is bearer-only. So prefix the server base to
/// give the relative path an origin and append the session token as a
/// `?token=` query param, the one auth an `<img>` fetch can carry. The
/// server accepts it via `MediaAuthUser` on the media read endpoints.
pub fn media_url(server_url: &str, path: &str) -> String {
    #[cfg(feature = "mobile")]
    {
        let base = format!("{server_url}{path}");
        match data::token_store::get() {
            Some(token) => format!("{base}?token={token}"),
            None => base,
        }
    }
    #[cfg(not(feature = "mobile"))]
    {
        let _ = server_url;
        path.to_string()
    }
}

/// Build the `/api/thumbs/{uuid}/{size}` responsive-thumbnail URL for one
/// size variant (`sm` / `md` / `lg`). Thin wrapper over [`media_url`] — call
/// it once per `srcset` entry so each candidate carries the mobile token.
pub fn thumb_url(server_url: &str, uuid: &str, size: &str) -> String {
    media_url(server_url, &format!("/api/thumbs/{uuid}/{size}"))
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

/// Browser-tab title. Owned by [`crate::App`] via `use_context_provider` and
/// rendered once as the App-level `document::Title`; each route sets its own
/// subtitle through [`use_page_title`]. Kept target-agnostic (mobile's WebView
/// has no visible tab, but the write is harmless and avoids a `cfg` gate).
#[derive(Copy, Clone)]
pub struct PageTitle(pub Signal<String>);

/// Set the browser-tab title to `Omnibus | {subtitle}` — or the bare app name
/// when `subtitle()` returns `None` (the landing page). `subtitle` is read
/// inside the effect, so a page whose name arrives asynchronously (book /
/// author / series detail) can pass a signal-backed closure and the title
/// re-renders once its data lands. Post-mount only, so SSR and the first WASM
/// paint keep the default title and hydration parity holds (rule 07).
pub fn use_page_title(subtitle: impl Fn() -> Option<String> + 'static) {
    let PageTitle(mut title) = use_context::<PageTitle>();
    use_effect(move || title.set(format_page_title(subtitle().as_deref())));
}

/// Compose the browser-tab title from an optional page subtitle.
fn format_page_title(subtitle: Option<&str>) -> String {
    match subtitle {
        Some(sub) => format!("Omnibus | {sub}"),
        None => "Omnibus".to_string(),
    }
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

/// Derive `is_admin` from the app-wide [`CurrentUser`] context — a pure
/// function of its value, so `use_memo` recomputes it inline with no extra
/// render pass. Web-only: mobile has no `CurrentUser` context to read, and
/// SSR renders this crate without the `web` feature at all, so both instead
/// return a `false`-valued signal — the same default the web memo evaluates
/// to on first paint (before the client's boot effect resolves `CurrentUser`),
/// keeping SSR/first-WASM-paint parity (rule 07). Returns a boxed
/// [`ReadSignal`] (rather than `Memo`/`Signal` directly) so call sites that
/// store the handle in a struct field see one type regardless of which of
/// the three arms below compiled.
#[cfg(feature = "web")]
pub fn use_is_admin() -> ReadSignal<bool> {
    let user_ctx = use_current_user().0;
    ReadSignal::new(use_memo(
        move || matches!(user_ctx(), Some(Some(ref u)) if u.is_admin),
    ))
}

/// Non-web fallback for [`use_is_admin`] — mobile has no `CurrentUser`
/// context and SSR never resolves one, so both stay at the `false` default.
#[cfg(not(feature = "web"))]
pub fn use_is_admin() -> ReadSignal<bool> {
    ReadSignal::new(use_signal(|| false))
}

/// Derive the resolved current user from the app-wide [`CurrentUser`]
/// context — a pure function of its value, so `use_memo` recomputes it
/// inline with no extra render pass, flattening "not yet resolved" and
/// "unauthenticated" alike to `None`. The memo evaluates to `None` on first
/// paint (before the client's boot effect resolves `CurrentUser`), matching
/// the SSR default and keeping hydration parity (rule 07). Returns a boxed
/// [`ReadSignal`], same reasoning as [`use_is_admin`].
#[cfg(feature = "web")]
pub fn use_current_user_summary() -> ReadSignal<Option<omnibus_shared::UserSummary>> {
    let user_ctx = use_current_user().0;
    ReadSignal::new(use_memo(move || user_ctx().flatten()))
}

/// Mobile fallback for [`use_current_user_summary`]. Mobile has no
/// `CurrentUser` context (bearer auth, not cookies), so — unlike the web
/// memo — this resolves the user via an async `/api/auth/me` fetch, which
/// isn't a pure derivation and so can't be a `use_memo`; it stays a
/// `use_signal` written from a post-mount `use_effect`, starting at `None`
/// to match the web/SSR default until the fetch lands. This is what lets
/// owner-only affordances (e.g. journal edit/delete) light up on mobile.
#[cfg(feature = "mobile")]
pub fn use_current_user_summary() -> ReadSignal<Option<omnibus_shared::UserSummary>> {
    let mut current = use_signal(|| None);
    let server_url = use_server_url();
    use_effect(move || {
        let server_url = server_url.clone();
        spawn(async move {
            if let Ok(user) = data::get_me(&server_url).await {
                current.set(Some(user));
            }
        });
    });
    ReadSignal::new(current)
}

/// SSR fallback for [`use_current_user_summary`] — neither `web` nor
/// `mobile`, so there's no context and no fetch; stays at the `None`
/// default that the web memo and mobile signal both start from.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub fn use_current_user_summary() -> ReadSignal<Option<omnibus_shared::UserSummary>> {
    ReadSignal::new(use_signal(|| None))
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
    /// The `book_files` row the picker selected (`?file_id=`). `None` lets the
    /// backend pick the lowest-ordinal audio file. Tracked as a signal so the
    /// App-level driver reboots when the *same* book's selected file changes.
    pub file_id: Signal<Option<i64>>,
    pub book: Signal<Option<omnibus_shared::EbookMetadata>>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    pub duration: Signal<f64>,
    pub elapsed: Signal<f64>,
    pub playing: Signal<bool>,
    pub rate: Signal<f64>,
    pub rate_error: Signal<Option<String>>,
    pub chapters: Signal<Vec<omnibus_shared::ChapterInfo>>,
    pub hls_ready: Signal<bool>,
    pub playback_failed: Signal<bool>,
    /// User's target volume (0.0–1.0), web-only UI. Session-wide, not reset
    /// on a book swap — the sleep-timer fade in `pages::listen::sleep`
    /// restores to this value rather than always `1.0`.
    pub volume: Signal<f64>,
}

#[cfg(not(feature = "mobile"))]
impl PlaybackState {
    /// Construct the initial (nothing-playing) playback state.
    pub fn new() -> Self {
        Self {
            uuid: Signal::new(None),
            file_id: Signal::new(None),
            book: Signal::new(None),
            loading: Signal::new(false),
            error: Signal::new(None),
            duration: Signal::new(0.0),
            elapsed: Signal::new(0.0),
            playing: Signal::new(false),
            rate: Signal::new(1.0),
            rate_error: Signal::new(None),
            chapters: Signal::new(Vec::new()),
            hls_ready: Signal::new(false),
            playback_failed: Signal::new(false),
            volume: Signal::new(1.0),
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

#[cfg(test)]
mod tests {
    use super::format_page_title;

    #[test]
    fn format_page_title_prefixes_subtitle_and_omits_when_none() {
        assert_eq!(format_page_title(Some("Settings")), "Omnibus | Settings");
        assert_eq!(format_page_title(None), "Omnibus");
    }
}
