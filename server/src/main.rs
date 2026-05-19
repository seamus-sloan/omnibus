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
            use omnibus_db::{
                indexer,
                worker::{Task, Worker},
            };
            use std::sync::Arc;

            // Capture process-start timestamp as the /api/_health build_id
            // before anything else can race the first probe. Lazy init from
            // the first request would label that timestamp "build_id" even
            // though the process has been up for seconds — misleading for
            // the rebuild-detection use case.
            backend::init_build_id();

            let database_url = std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());

            let pool = omnibus_db::init_db(&database_url).await?;
            omnibus_db::seed_settings_from_env(&pool).await?;

            // Recovery hook: promote the named user to admin if
            // OMNIBUS_INITIAL_ADMIN is set. No-op otherwise. Logs on
            // promotion so the action is auditable.
            auth::boot::apply_initial_admin(&pool).await?;

            // Dev convenience: create OMNIBUS_DEV_SEED_USER if set and the
            // user doesn't yet exist. Sourced from `.env` (gitignored) via
            // the flake.nix shellHook — production never sets it. Logs on
            // seed so any stray prod-env occurrence is loud.
            auth::boot::seed_dev_user(&pool).await?;

            let state = backend::AppState::new(pool.clone());
            let worker: Arc<Worker> = state.worker().clone();

            // Kick off a reindex through the shared worker if the index is
            // empty or stale. The first user request reads whatever is
            // currently in the DB; the refresh flows in next time the page
            // loads. Treat read errors as "stale" so a malformed timestamp
            // doesn't silently suppress the recovery scan.
            if let Ok(settings) = omnibus_db::get_settings(&pool).await {
                if let Some(path) = settings.ebook_library_path {
                    let stale = indexer::is_stale(&pool, &path).await.unwrap_or(true);
                    if stale {
                        worker.post(Task::Scan { library_path: path });
                    }
                }
            }

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
                .layer(Extension(worker))
                .layer(tower_http::trace::TraceLayer::new_for_http());

            Ok(router)
        });
    }
}
