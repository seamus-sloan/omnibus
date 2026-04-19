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
            use omnibus::backend;
            use omnibus_frontend::ebook_cache::{self, EbookCache};

            let database_url = std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());

            let pool = omnibus_frontend::db::init_db(&database_url).await?;
            omnibus_frontend::db::seed_settings_from_env(&pool).await?;

            let ebook_cache = EbookCache::default();

            // Prime the cache in the background so the first user request
            // is likely to hit a warm cache instead of waiting for the
            // full filesystem walk + OPF parse.
            if let Ok(settings) = omnibus_frontend::db::get_settings(&pool).await {
                let cache_for_warm = ebook_cache.clone();
                tokio::spawn(async move {
                    let _ = ebook_cache::load_or_scan(&cache_for_warm, settings.ebook_library_path)
                        .await;
                });
            }

            let state = backend::AppState::new(pool.clone(), ebook_cache.clone());
            let router = dioxus::server::router(App)
                .merge(backend::rest_router(state))
                .layer(Extension(pool))
                .layer(Extension(ebook_cache))
                .layer(tower_http::trace::TraceLayer::new_for_http());

            Ok(router)
        });
    }
}
