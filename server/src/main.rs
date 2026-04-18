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

            let database_url = std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://omnibus.db?mode=rwc".to_string());

            let pool = omnibus_frontend::db::init_db(&database_url).await?;
            omnibus_frontend::db::seed_settings_from_env(&pool).await?;

            let state = backend::AppState::new(pool.clone());
            let router = dioxus::server::router(App)
                .merge(backend::rest_router(state))
                .layer(Extension(pool))
                .layer(tower_http::trace::TraceLayer::new_for_http());

            Ok(router)
        });
    }
}
