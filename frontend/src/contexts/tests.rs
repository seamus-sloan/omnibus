//! Unit tests for the app-wide contexts: page-title formatting, the cover
//! cache-bust URL helpers and their shared signal, and the non-web fallback
//! defaults of the `use_*` accessor hooks.

use std::collections::HashMap;

use dioxus::prelude::*;

use super::*;

#[test]
fn format_page_title_prefixes_subtitle_and_omits_when_none() {
    assert_eq!(format_page_title(Some("Settings")), "Omnibus | Settings");
    assert_eq!(format_page_title(None), "Omnibus");
}

#[test]
fn append_cache_bust_is_a_no_op_when_bust_is_zero() {
    assert_eq!(
        append_cache_bust("/api/covers/abc".into(), 0),
        "/api/covers/abc"
    );
}

#[test]
fn append_cache_bust_adds_query_param_when_bust_is_nonzero() {
    assert_eq!(
        append_cache_bust("/api/covers/abc".into(), 2),
        "/api/covers/abc?v=2"
    );
}

#[test]
fn append_cache_bust_uses_ampersand_when_url_already_has_a_query_string() {
    // Mirrors mobile's `media_url`, which appends `?token=…`.
    assert_eq!(
        append_cache_bust("/api/covers/abc?token=xyz".into(), 1),
        "/api/covers/abc?token=xyz&v=1"
    );
}

// Regression for #1087: after `CoverEditor` bumps a book's counter, every
// other reader of the same `CoverCacheBust` signal must observe it — this
// is what lets the landing grid/table and book detail pick up a cover
// change made on a page they've already left, without a manual refresh.
// `Signal::new` requires an active Dioxus runtime, so the assertions run
// inside a throwaway component driven by a bare `VirtualDom` rebuild
// (mirrors `dioxus::ssr::render_element`'s use elsewhere in this crate
// for exercising component-only APIs from a plain `#[test]`).
#[test]
fn bump_cover_cache_bust_is_observed_by_other_readers_of_the_same_signal() {
    #[component]
    fn AssertBustCounter() -> Element {
        let bust: Signal<HashMap<String, u32>> = Signal::new(HashMap::new());
        assert_eq!(cover_bust_for(bust, "book-1"), 0);

        bump_cover_cache_bust(bust, "book-1");
        assert_eq!(cover_bust_for(bust, "book-1"), 1);

        // A second bump (e.g. upload then revert) keeps incrementing so a
        // stale cached URL from either step is still invalidated.
        bump_cover_cache_bust(bust, "book-1");
        assert_eq!(cover_bust_for(bust, "book-1"), 2);

        // A different book's counter is untouched.
        assert_eq!(cover_bust_for(bust, "book-2"), 0);

        rsx! {}
    }
    VirtualDom::new(AssertBustCounter).rebuild_in_place();
}

// The `web` arm (which derives the real value from `CurrentUser`) needs
// the wasm32 target and isn't compiled for this native run, so only the
// non-web fallback is exercisable here — same blind spot `use_is_admin`
// already has. It's still worth pinning: this is the value SSR and the
// first WASM paint both render before the boot effect resolves the real
// permission (rule 07), so a regression here would flash the upload UI.
#[test]
fn use_can_upload_defaults_to_false_on_the_non_web_fallback() {
    #[component]
    fn AssertCanUpload() -> Element {
        let can_upload = use_can_upload();
        assert!(!can_upload());
        rsx! {}
    }
    VirtualDom::new(AssertCanUpload).rebuild_in_place();
}

// Same blind spot as `use_can_upload`'s test: the `web` arm (deriving
// `is_admin` from `CurrentUser` via `use_memo`) needs the wasm32 target,
// so only the non-web fallback compiles into this native test run. This
// is the value SSR and the first WASM paint both render before the boot
// effect resolves the real permission (rule 07) — a regression here
// would flash admin-only affordances (author Delete, landing inline
// edits) to every visitor.
#[test]
fn use_is_admin_defaults_to_false_on_the_non_web_fallback() {
    #[component]
    fn AssertIsAdmin() -> Element {
        let is_admin = use_is_admin();
        assert!(!is_admin());
        rsx! {}
    }
    VirtualDom::new(AssertIsAdmin).rebuild_in_place();
}

// This native `server`-feature test run compiles neither the `web` arm
// (derives from `CurrentUser` via `use_memo`) nor the `mobile` arm
// (fetches `/api/auth/me` in an effect) of `use_current_user_summary` —
// only the SSR fallback. It's still worth pinning: it's the value every
// owner-only affordance (journal edit/delete) starts from before the
// real resolution lands, on every target (rule 07).
#[test]
fn use_current_user_summary_defaults_to_none_on_the_ssr_fallback() {
    #[component]
    fn AssertCurrentUserSummary() -> Element {
        let user = use_current_user_summary();
        assert_eq!(user(), None);
        rsx! {}
    }
    VirtualDom::new(AssertCurrentUserSummary).rebuild_in_place();
}

// `use_server_url`'s mobile arm reads a reactive context signal and
// isn't compiled into this non-mobile test run; the non-mobile arm below
// is a plain function with no hooks, so it needs no Dioxus runtime at
// all — web/SSR co-locate with the server, so every media/API call is
// same-origin and relative. Gated to match: under `mobile`, calling the
// context-reading arm outside a live `VirtualDom` panics.
#[cfg(not(feature = "mobile"))]
#[test]
fn use_server_url_is_empty_on_the_non_mobile_target() {
    assert_eq!(use_server_url(), "");
}

// `media_url`'s mobile arm (token-proxied, offline-cache-aware) isn't
// compiled into this non-mobile test run; the non-mobile arm is the one
// every web/SSR render actually uses. Gated to match the arm it asserts.
#[cfg(not(feature = "mobile"))]
#[test]
fn media_url_returns_the_path_unchanged_on_the_non_mobile_target() {
    assert_eq!(
        media_url("http://example.com", "/api/covers/abc"),
        "/api/covers/abc"
    );
}

// `thumb_url` delegates to `media_url`, so it inherits the same
// mobile-vs-non-mobile split and gate.
#[cfg(not(feature = "mobile"))]
#[test]
fn thumb_url_builds_the_sized_thumbnail_path() {
    assert_eq!(
        thumb_url("http://example.com", "book-uuid", "md"),
        "/api/thumbs/book-uuid/md"
    );
}

// `use_search_query`/`use_cover_cache_bust`/`use_current_user`/
// `use_playback` are one-line `use_context` accessors; the meaningful
// behaviour they expose (`cover_bust_for`/`bump_cover_cache_bust`,
// `PlaybackState::new`'s defaults) already has direct coverage above and
// in the `PlaybackState` doc example. This test pins the accessors
// themselves: each must resolve to the exact value the provider handed
// it, so a future refactor that reaches for the wrong context type still
// fails loudly here rather than only inside a much larger page test.
// `CurrentUser`/`PlaybackState`/`use_current_user`/`use_playback` are
// themselves `#[cfg(not(feature = "mobile"))]`, so this test must match.
#[cfg(not(feature = "mobile"))]
#[test]
fn context_accessors_resolve_to_the_values_their_providers_set() {
    #[component]
    fn AssertAccessors() -> Element {
        use_context_provider(|| SearchQuery(Signal::new("dune".to_string())));
        use_context_provider(|| {
            CoverCacheBust(Signal::new(HashMap::from([("book-1".to_string(), 3u32)])))
        });
        use_context_provider(|| CurrentUser(Signal::new(None)));
        use_context_provider(PlaybackState::new);

        assert_eq!(use_search_query().0(), "dune");
        assert_eq!(cover_bust_for(use_cover_cache_bust().0, "book-1"), 3);
        assert_eq!(use_current_user().0(), None);
        assert_eq!((use_playback().rate)(), 1.0);
        rsx! {}
    }
    VirtualDom::new(AssertAccessors).rebuild_in_place();
}
