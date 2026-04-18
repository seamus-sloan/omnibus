//! Server functions callable from the web client.
//!
//! Each `#[get]`/`#[post]` function is compiled in two modes:
//! - On the server (`feature = "server"`) the body executes, accessing the
//!   SQLite pool via an `axum::Extension<SqlitePool>` layered onto the
//!   Dioxus fullstack router in `server/src/main.rs`.
//! - On the web client (`feature = "web"`) the macro generates a fetch-based
//!   stub callable as a normal async function.
//!
//! Mobile does **not** use these. It talks to the hand-written `/api/*`
//! REST routes (see `server/src/backend.rs`) via `reqwest`.
//!
//! These routes are distinct from the REST routes (`/api/rpc/*` vs `/api/*`)
//! so the two clients cannot accidentally collide.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{LibraryContents, Settings, ValueResponse};

#[cfg(feature = "server")]
use crate::{db, scanner};

/// Server-only extractor alias used by each server function. Only referenced
/// by the server-side body; the `#[cfg(feature = "server")]` stops the web
/// build from importing axum/sqlx types.
#[cfg(feature = "server")]
type PoolExt = dioxus::fullstack::axum::Extension<sqlx::SqlitePool>;

#[get("/api/rpc/value", pool: PoolExt)]
pub async fn rpc_get_value() -> Result<ValueResponse> {
    let value = db::get_value(&pool.0).await?;
    Ok(ValueResponse { value })
}

#[post("/api/rpc/value/increment", pool: PoolExt)]
pub async fn rpc_increment_value() -> Result<ValueResponse> {
    let value = db::increment_value(&pool.0).await?;
    Ok(ValueResponse { value })
}

#[get("/api/rpc/settings", pool: PoolExt)]
pub async fn rpc_get_settings() -> Result<Settings> {
    Ok(db::get_settings(&pool.0).await?)
}

#[post("/api/rpc/settings", pool: PoolExt)]
pub async fn rpc_save_settings(settings: Settings) -> Result<Settings> {
    db::set_settings(&pool.0, &settings).await?;
    Ok(db::get_settings(&pool.0).await?)
}

#[get("/api/rpc/library", pool: PoolExt)]
pub async fn rpc_get_library() -> Result<LibraryContents> {
    let settings = db::get_settings(&pool.0).await?;
    Ok(scanner::scan_libraries(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    ))
}
