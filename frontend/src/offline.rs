//! Offline-first layer for the mobile client: a durable SQLite store
//! (replica cache + mutation outbox + download registry), a loopback media
//! server for local playback, a download engine, and the sync engine that
//! drains queued writes on reconnect. Compiled only under
//! `feature = "mobile"` — web/SSR builds never see this module.

pub mod cache;
pub mod downloads;
pub mod media;
pub mod outbox;
pub mod replica;
pub mod store;
pub mod sync;

/// Initialize the offline layer. Called once from the native shell's
/// `main()` before `dioxus::launch`, right after `token_store::load_from_disk`.
///
/// Best-effort by design: if the app data dir or SQLite open fails, every
/// offline API degrades to a no-op and the app behaves exactly as the
/// online-only build did (matching `app_dirs::data_dir() == None` semantics).
pub fn init() {
    store::init_global();
    downloads::init();
    media::start();
    // Cold-start resume positions: mirror the durable rows back into the
    // in-memory maps the reader/player read synchronously.
    crate::reader_progress::hydrate_from_offline_store();
    crate::audiobook_progress::hydrate_from_offline_store();
}

/// Cache-key prefixes holding *user-scoped* replicated data. Library-wide
/// rows (books, authors, series, manifests, the replica) survive an account
/// switch; these do not.
const USER_SCOPED_PREFIXES: [&str; 16] = [
    "me",
    "progress:",
    "recent_progress",
    "rate:",
    "highlights:",
    "bookmarks:",
    "journals:",
    "rating:",
    "ratings_others:",
    "shelves",
    "shelf:",
    "shelf_page:",
    "stats:",
    "reader_cfi:",
    "audio_pos:",
    "audio_rate:",
];

/// Record which account this device's replica belongs to; when a *different*
/// user signs in, wipe the previous user's replicated data and queued
/// mutations so nothing leaks across accounts (books/covers stay — the
/// library is server-wide).
///
/// Takes the fresh `me` the caller just fetched (rather than a bare
/// username) so that, on a detected switch, the wipe below — whose
/// `kv_delete_prefix` over `USER_SCOPED_PREFIXES` matches the `"me"` cache
/// key — can be followed by re-seeding that same row. Without this, the
/// prior cache write `get_me` made for the new user's identity would be
/// deleted by this function's own wipe, forcing a redundant network
/// re-fetch on the very next read.
pub(crate) async fn note_user(user: &omnibus_shared::UserSummary) {
    let Some(st) = store::store() else { return };
    let previous = st.meta_get("last_user").await;
    match previous.as_deref() {
        Some(prev) if prev == user.username => {}
        Some(_) => {
            tracing::info!("account switch detected; clearing user-scoped offline data");
            for prefix in USER_SCOPED_PREFIXES {
                st.kv_delete_prefix(prefix);
            }
            let ids: Vec<i64> = st.ops_list().await.into_iter().map(|o| o.id).collect();
            st.ops_delete_many(ids).await;
            sync::refresh_pending().await;
            st.meta_put("last_user", user.username.clone());
            // Restore the identity row the wipe above just deleted.
            cache::put_json(&cache::keys::me(), user);
        }
        None => st.meta_put("last_user", user.username.clone()),
    }
}

#[cfg(test)]
mod tests;
