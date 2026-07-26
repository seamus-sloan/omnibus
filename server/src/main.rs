//! Unified Dioxus fullstack entrypoint. Built for WASM, `main` calls
//! `dioxus::launch` to hydrate the client; built natively (`server`
//! feature), it calls `dioxus::serve` to run an Axum backend serving SSR'd
//! HTML, the WASM bundle, [`omnibus_frontend::rpc`] server functions, and
//! [`omnibus::backend`]'s mobile-facing REST routes.

use omnibus_frontend::App;

fn main() {
    #[cfg(not(feature = "server"))]
    {
        dioxus::launch(App);
    }

    #[cfg(feature = "server")]
    {
        // Bind the appender guard for the whole process: dropping it flushes
        // the non-blocking file writer's buffer. `dioxus::serve` blocks until
        // shutdown, so the guard lives exactly as long as the server does.
        let _log_guard = omnibus::logging::init_tracing();
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
    use omnibus::{auth, backend, metrics, rate_limit, security_headers};
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
        spawn_periodic_scan(pool.clone(), worker.clone());

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
        // Read OMNIBUS_VERSION once at boot so /api/_health's `version`
        // field reflects the process's own launch env, not a value that
        // could drift if something mutated the env var mid-run (#1055).
        backend::init_app_version();
    }

    /// Log a WARN if `OMNIBUS_TRUST_FORWARDED_FOR` is enabled — required only
    /// behind a trusted reverse proxy, dangerous otherwise — and a one-time
    /// WARN if kepubify is missing (Kobo downloads then fall back to plain
    /// EPUB).
    fn log_startup_warnings() {
        if rate_limit::trust_forwarded_for() {
            tracing::warn!(
                target: "omnibus::startup",
                "OMNIBUS_TRUST_FORWARDED_FOR is enabled \u{2014} ensure a trusted reverse proxy is in front."
            );
        }
        omnibus_db::kepub::warn_if_unavailable();
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
        // Seed the Google Books key from GOOGLE_BOOKS_API_KEY only when none is
        // saved (settings wins; env is the out-of-the-box fallback).
        omnibus_db::seed_google_books_key_from_env(&pool).await?;
        // Seed the F4.3 SMTP config from SMTP_* only when no host is saved
        // (settings wins; env is the out-of-the-box fallback).
        omnibus_db::seed_smtp_from_env(&pool).await?;

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

    /// Spawn the configurable periodic library rescan (F: "Configurable
    /// Periodic Library Scan Interval"). Mirrors `spawn_session_pruner`'s
    /// loop-forever shape, but the wait before each tick is *not* fixed at
    /// spawn time: `periodic_scan_tick` re-reads `scan_interval_hours` from
    /// settings every iteration and reports how long to sleep next, so a
    /// settings change (including disabling it) takes effect on the next
    /// tick without a restart, and the first tick fires immediately.
    fn spawn_periodic_scan(pool: SqlitePool, worker: Arc<Worker>) {
        tokio::spawn(async move {
            loop {
                let wait = omnibus_db::worker::periodic_scan_tick(&pool, &worker).await;
                tokio::time::sleep(wait).await;
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

        // Prometheus HTTP metrics: middleware + `/metrics` scrape route. The
        // layer goes on outermost (see below); the route merges in here.
        let (prometheus_layer, metrics_route) = metrics::layer_and_route();

        dioxus::server::router(App)
            .merge(metrics_route)
            .layer(axum::middleware::from_fn_with_state(
                (search_limiter.clone(), search_rpc_prefixes),
                rate_limit::rate_limit_paths,
            ))
            .merge(backend::rest_router_with_search_limiter(
                state.clone(),
                search_limiter,
            ))
            // Native Kobo wireless sync. Sits outside the `/api/*` auth gate
            // (require_auth passes through non-`/api/` paths); each route
            // authenticates via its path token internally.
            .merge(backend::kobo_router(state.clone()))
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
            // Outermost app layer so the histograms observe every request
            // (including the 408/413 short-circuits from the guards above).
            .layer(prometheus_layer)
    }

    /// Layer global HTTP security response headers onto `router`, plus
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
        // TraceLayer last so it is the outermost layer and observes every
        // response — including 408/413 short-circuits from the timeout and
        // body-limit guards above. Span logs only the path, never the query
        // string: media reads carry the session as `?token=`, and the default
        // span records the full URI — which would leak live tokens into logs.
        // The span and the on-response event sit at INFO so the default
        // filter yields one line per request (method, path, status, latency).
        router.layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|req: &axum::http::Request<_>| {
                    tracing::info_span!(
                        "request",
                        method = %req.method(),
                        path = %redact_path(req.uri().path()),
                        version = ?req.version(),
                    )
                })
                .on_response(
                    tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO),
                ),
        )
    }

    /// Redact the `<TOKEN>` segment of a `/kobo/<TOKEN>/…` path before it
    /// reaches the trace span. Kobo devices carry a long-lived (90-day)
    /// session token directly in the URL path (see
    /// `backend::kobo::extractor::kobo_path_token`), so logging the raw path
    /// would leak a durable credential to stderr and the on-disk JSON sink.
    /// Every other path is returned unchanged, borrowed rather than
    /// allocated — this runs on every request, and only the Kobo case needs
    /// to build a new string.
    fn redact_path(path: &str) -> std::borrow::Cow<'_, str> {
        let mut segs = path.split('/');
        match (segs.next(), segs.next(), segs.next()) {
            (Some(""), Some("kobo"), Some(_token)) => {
                let rest: String = segs.map(|s| format!("/{s}")).collect();
                std::borrow::Cow::Owned(format!("/kobo/[REDACTED]{rest}"))
            }
            _ => std::borrow::Cow::Borrowed(path),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::redact_path;

        #[test]
        fn redact_path_replaces_the_token_segment_of_a_kobo_path() {
            assert_eq!(
                redact_path("/kobo/abc123/v1/library/sync"),
                "/kobo/[REDACTED]/v1/library/sync"
            );
            assert_eq!(
                redact_path("/kobo/abc123/v1/download/some-uuid"),
                "/kobo/[REDACTED]/v1/download/some-uuid"
            );
        }

        #[test]
        fn redact_path_leaves_non_kobo_paths_unchanged() {
            assert_eq!(redact_path("/api/ebooks"), "/api/ebooks");
            assert_eq!(redact_path("/"), "/");
            assert_eq!(redact_path("/kobo"), "/kobo");
        }
    }
}
