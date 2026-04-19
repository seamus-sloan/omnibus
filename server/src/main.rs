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
            use omnibus_frontend::indexer;

            let database_url = std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());

            let pool = omnibus_frontend::db::init_db(&database_url).await?;
            omnibus_frontend::db::seed_settings_from_env(&pool).await?;

            // Kick off a reindex in the background if the index is empty or
            // stale. The first user request reads whatever is currently in
            // the DB; the refresh flows in next time the page loads.
            if let Ok(settings) = omnibus_frontend::db::get_settings(&pool).await {
                if let Some(path) = settings.ebook_library_path {
                    indexer::spawn_reindex_if_stale(pool.clone(), path);
                }
            }

            let state = backend::AppState::new(pool.clone());
            let router = dioxus::server::router(App)
                .merge(backend::rest_router(state))
                .layer(Extension(pool))
                .layer(tower_http::trace::TraceLayer::new_for_http());

            Ok(router)
        });
    }
}
