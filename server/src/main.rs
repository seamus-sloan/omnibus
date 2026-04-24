//! Unified Dioxus fullstack entrypoint.
//!
//! - When built for WASM (no `server` feature), `main` calls `dioxus::launch`
//!   to hydrate the client in the browser.
//! - When built natively (`server` feature), `main` calls `dioxus::serve` to
//!   run an Axum backend that serves SSR'd HTML, the WASM bundle, the
//!   auto-registered `#[get]`/`#[post]` server functions from
//!   [`omnibus_frontend::rpc`], and the hand-written `/api/*` REST routes
//!   from [`omnibus::backend`] (mobile-facing).

use omnibus_frontend::App;

fn main() {
    #[cfg(not(feature = "server"))]
    {
        dioxus::launch(App);
    }

    #[cfg(feature = "server")]
    {
        dioxus::serve(|| async move {
            use dioxus::server::axum::Extension;
            use omnibus::{auth, backend};
            use omnibus_db::indexer;
            use std::sync::Arc;

            let database_url = std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());

            let pool = omnibus_db::init_db(&database_url).await?;
            omnibus_db::seed_settings_from_env(&pool).await?;

            // Recovery hook: promote the named user to admin if
            // OMNIBUS_INITIAL_ADMIN is set. No-op otherwise. Logs on
            // promotion so the action is auditable.
            auth::boot::apply_initial_admin(&pool).await?;

            // Kick off a reindex in the background if the index is empty or
            // stale. The first user request reads whatever is currently in
            // the DB; the refresh flows in next time the page loads.
            if let Ok(settings) = omnibus_db::get_settings(&pool).await {
                if let Some(path) = settings.ebook_library_path {
                    indexer::spawn_reindex_if_stale(pool.clone(), path);
                }
            }

            let state = backend::AppState::new(pool.clone());
            let limiter = Arc::new(auth::RateLimiter::new());
            let router = dioxus::server::router(App)
                .merge(backend::rest_router(state.clone()))
                .merge(auth::auth_router(state.clone()).layer(
                    axum::middleware::from_fn_with_state(limiter, auth::rate_limit_auth),
                ))
                // Apply require_auth and origin_check at the top level so
                // every cookie-authed /api/* request — not just /api/auth/* —
                // is origin-checked. Bearer requests and safe methods are
                // exempt inside origin_check; non-cookie requests short-circuit
                // there too, so SSR and static assets pass through unchanged.
                .layer(axum::middleware::from_fn_with_state(
                    state,
                    auth::require_auth,
                ))
                .layer(axum::middleware::from_fn(auth::origin_check))
                .layer(Extension(pool))
                .layer(tower_http::trace::TraceLayer::new_for_http());

            Ok(router)
        });
    }
}
