//! Route-driven effects for [`super::MobilePlayer`]: retargeting the
//! app-wide playback context to the current route's book, and refreshing
//! the title marquee measurement.

use dioxus::prelude::*;
use dioxus_router::Navigator;

use super::interop;
use super::state::MobilePlayback;
use super::view::PlayerView;
use crate::Route;

/// Point the app-wide player at `route_uuid`/`file_id` — only when it
/// differs, so re-entering the currently-playing book (e.g. from the
/// mini-player) is seamless instead of restarting the surface. Offline with
/// no completed audio download, retargeting would only stall, so this
/// bounces back with the app-level offline sheet instead (mirrors the
/// reader guard); an already-loaded book skips that check. Split out of
/// [`super::MobilePlayer`] as its own hook (call-order-stable, so it's
/// still safe to invoke unconditionally from the component body).
pub(super) fn use_retarget_playback(
    route_uuid: String,
    file_id: Option<i64>,
    ctx: MobilePlayback,
    nav: Navigator,
) {
    use_effect(use_reactive!(|(route_uuid, file_id)| {
        let mut uuid_sig = ctx.uuid;
        let mut file_sig = ctx.file_id;
        let already_loaded = uuid_sig.peek().as_deref() == Some(route_uuid.as_str());
        if !already_loaded
            && crate::offline::sync::is_offline()
            && !crate::offline::downloads::is_complete(
                &route_uuid,
                crate::offline::downloads::DlFormat::Audio,
            )
        {
            crate::components::offline_guard::block(
                "This audiobook isn\u{2019}t downloaded, so it can\u{2019}t be played while offline.",
            );
            if nav.can_go_back() {
                nav.go_back();
            } else {
                nav.replace(Route::BookDetail {
                    uuid: route_uuid.clone(),
                });
            }
            return;
        }
        // Publish the picker's selection first so the host reads the right file
        // when the uuid change kicks off the load.
        file_sig.set(file_id);
        if uuid_sig.peek().as_deref() != Some(route_uuid.as_str()) {
            uuid_sig.set(Some(route_uuid.clone()));
        }
    }));
}

/// Re-measure the title marquee whenever the displayed title (re)appears —
/// covers the loading→loaded transition and any book switch. Split out of
/// [`super::MobilePlayer`] as its own hook (call-order-stable, so it's
/// still safe to invoke unconditionally from the component body).
pub(super) fn use_marquee_title_refresh(view_now: &Option<PlayerView>) {
    let marquee_title = view_now.as_ref().map(|v| v.title.clone());
    use_effect(use_reactive!(|marquee_title| {
        if marquee_title.is_some() {
            interop::refresh_title_marquee();
        }
    }));
}
