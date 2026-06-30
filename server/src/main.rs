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
        dioxus::serve(server::launch);
    }
}

/// Native fullstack startup. Gated behind the `server` feature so the WASM
/// build never pulls these helpers (and their tokio + sqlx deps) into the
/// browser bundle.
#[cfg(feature = "server")]
mod server {
    use std::sync::Arc;

    use axum::Router;
    use dioxus::server::axum::Extension;
    use omnibus::{auth, backend, rate_limit, security_headers};
    use omnibus_db::{
        indexer,
        worker::{Task, Worker},
    };
    use sqlx::SqlitePool;

    use crate::App;

    /// Entry point handed to `dioxus::serve`: boots the stack and returns the wired Axum `Router`.
    pub(crate) async fn launch() -> anyhow::Result<Router> {
        init_boot_metadata();
        log_startup_warnings();

        let pool = init_database().await?;
        let state = backend::AppState::new(pool.clone());
        let worker: Arc<Worker> = state.worker().clone();

        kick_recovery_scans(&pool, &worker).await;
        spawn_session_pruner(pool.clone());

        let router = build_router(state, pool, worker);
        Ok(apply_security_headers(router))
    }

    /// Capture process-start timestamp + repo root before any request can race
    /// the first `/api/_health` probe. Lazy init from the first request would
    /// label the build_id seconds-after-boot, which is misleading for the
    /// rebuild-detection use case.
    fn init_boot_metadata() {
        backend::init_build_id();
        // scripts/dev-server-up.sh uses repo_root to distinguish "my
        // workspace's server" from a sibling workspace's server bound to the
        // port I want.
        backend::init_repo_root();
    }

    /// Log a WARN if `OMNIBUS_TRUST_FORWARDED_FOR` is enabled — required only
    /// behind a trusted reverse proxy, dangerous otherwise.
    fn log_startup_warnings() {
        if rate_limit::trust_forwarded_for() {
            tracing::warn!(
                target: "omnibus::startup",
                "OMNIBUS_TRUST_FORWARDED_FOR is enabled \u{2014} ensure a trusted reverse proxy is in front."
            );
        }
    }

    /// Open the SQLite pool, run migrations, seed env-driven settings, then
    /// apply the boot-time admin / dev-user hooks.
    async fn init_database() -> anyhow::Result<SqlitePool> {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());

        let pool = omnibus_db::init_db(&database_url).await?;
        omnibus_db::seed_settings_from_env(&pool).await?;
        // Seed the F3.3 Hardcover key from HARDCOVER_API_KEY only when none is
        // saved (settings wins; env is the out-of-the-box fallback).
        omnibus_db::seed_hardcover_key_from_env(&pool).await?;

        // Recovery hook: promote the named user to admin if
        // OMNIBUS_INITIAL_ADMIN is set. No-op otherwise. Logs on
        // promotion so the action is auditable.
        auth::boot::apply_initial_admin(&pool).await?;

        // Dev convenience: create OMNIBUS_DEV_SEED_USER if set and the
        // user doesn't yet exist. Sourced from `.env` (gitignored) via
        // the flake.nix shellHook — production never sets it. Logs on
        // seed so any stray prod-env occurrence is loud.
        auth::boot::seed_dev_user(&pool).await?;

        Ok(pool)
    }

    /// Kick off a reindex through the shared worker if the index is empty or
    /// stale. The first user request reads whatever is currently in the DB;
    /// the refresh flows in next time the page loads. Treat read errors as
    /// "stale" so a malformed timestamp doesn't silently suppress the
    /// recovery scan.
    async fn kick_recovery_scans(pool: &SqlitePool, worker: &Arc<Worker>) {
        if let Ok(settings) = omnibus_db::get_settings(pool).await {
            if let Some(path) = settings.ebook_library_path {
                let stale = indexer::is_stale(pool, &path).await.unwrap_or(true);
                if stale {
                    worker.post(Task::Scan { library_path: path });
                }
            }
            if let Some(path) = settings.audiobook_library_path {
                let stale = indexer::is_stale(pool, &path).await.unwrap_or(true);
                if stale {
                    worker.post(Task::ScanAudiobooks { library_path: path });
                }
            }
        }
    }

    /// Spawn a daily background task that prunes sessions which can never
    /// authenticate again (revoked, absolute-expired, or idle-expired). The
    /// first interval tick fires immediately, so this also runs at boot.
    fn spawn_session_pruner(pool: SqlitePool) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
            loop {
                interval.tick().await;
                match omnibus_db::auth::prune_expired_sessions(&pool).await {
                    Ok(n) if n > 0 => {
                        tracing::info!(pruned = n, "pruned expired/revoked/idle sessions");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "session prune failed");
                    }
                }
            }
        });
    }

    /// Assemble the full Axum router: RPC search rate-limit, REST router,
    /// auth router (with its own credential-handling rate-limit), then the
    /// auth / origin-check / extensions / timeout / body-limit middleware
    /// stack. Returns the router; security headers and the trace layer are
    /// added by `apply_security_headers`.
    fn build_router(state: backend::AppState, pool: SqlitePool, worker: Arc<Worker>) -> Router {
        let auth_limiter = Arc::new(rate_limit::RateLimiter::new());
        // Prefix-scope the auth limiter to credential-handling endpoints
        // only. `/api/auth/me` is an authenticated read of the caller's
        // own row — it's hit on every web App boot (and historically
        // on every page mount) but presents no brute-force surface, so
        // sharing the same 10-req/60s bucket as `/login`/`/register`
        // just throttled legitimate UI rendering (and parallel
        // Playwright workers from the same loopback IP). Logout stays
        // limited because a stolen token could otherwise be used to
        // DoS revoke endpoints.
        let auth_limiter_prefixes: Arc<Vec<&'static str>> = Arc::new(vec![
            "/api/auth/login",
            "/api/auth/register",
            "/api/auth/logout",
        ]);
        // One per-IP budget shared by the REST `/api/search/*` and RPC
        // `/api/rpc/search-*` layers (same Arc), so neither reaches 2× (#249).
        let search_limiter = Arc::new(rate_limit::RateLimiter::with_policy(
            backend::SEARCH_RATE_LIMIT_WINDOW,
            backend::SEARCH_RATE_LIMIT_MAX,
        ));
        // `starts_with` prefix covers both `/api/rpc/search` and
        // `/api/rpc/search-palette` so neither bypasses the budget (#249).
        let search_rpc_prefixes: Arc<Vec<&'static str>> = Arc::new(vec!["/api/rpc/search"]);

        dioxus::server::router(App)
            .layer(axum::middleware::from_fn_with_state(
                (search_limiter.clone(), search_rpc_prefixes),
                rate_limit::rate_limit_paths,
            ))
            .merge(backend::rest_router_with_search_limiter(
                state.clone(),
                search_limiter,
            ))
            .merge(
                auth::auth_router(state.clone()).layer(axum::middleware::from_fn_with_state(
                    (auth_limiter, auth_limiter_prefixes),
                    rate_limit::rate_limit_paths,
                )),
            )
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
            // Global request-handling guards. A slow client or oversized
            // body can otherwise hold a tokio worker indefinitely.
            //
            // - `TimeoutLayer` aborts any request that takes longer than
            //   30s end-to-end. Long-running scans run on the background
            //   `Worker` (not the request task), so 30s is safely above
            //   any synchronous handler.
            // - `DefaultBodyLimit` caps request bodies to 1 MiB by
            //   default. Routes that legitimately need larger payloads
            //   (e.g. `POST /api/ebooks/{id}/cover`) layer their own
            //   `DefaultBodyLimit::max(...)` closer to the handler,
            //   which takes precedence over this outer cap.
            .layer(tower_http::timeout::TimeoutLayer::with_status_code(
                axum::http::StatusCode::REQUEST_TIMEOUT,
                std::time::Duration::from_secs(30),
            ))
            .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
    }

    /// Layer global HTTP security response headers (#277) onto `router`, plus
    /// the optional HSTS layer (only when secure cookies are enabled) and the
    /// outermost trace layer. Separate fold because `SetResponseHeaderLayer`
    /// is one layer per header; placed outside the timeout/body-limit guards
    /// but inside the trace layer so headers attach to every response —
    /// including the 408/413 short-circuits emitted by those guards.
    fn apply_security_headers(router: Router) -> Router {
        let secure_cookies = auth::handlers::parse_secure_cookies(
            std::env::var("OMNIBUS_SECURE_COOKIES").ok().as_deref(),
        );
        let mut router = router;
        for layer in security_headers::baseline_layers() {
            router = router.layer(layer);
        }
        if let Some(layer) = security_headers::hsts_layer(secure_cookies) {
            router = router.layer(layer);
        }
        // TraceLayer last so it is the outermost layer and observes
        // every response — including 408/413 short-circuits from the
        // timeout and body-limit guards above.
        router.layer(tower_http::trace::TraceLayer::new_for_http())
    }
}
