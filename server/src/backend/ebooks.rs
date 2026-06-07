//! `/api/ebooks/*` handlers.
//!
//! Cookie-gated reads that list the configured library, look up a single
//! book by uuid, and stream the raw EPUB bytes for the in-app reader.
//! Mounted on the mobile REST router in [`super::rest_router`].

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, scanner};

use super::{internal, with_pagination_headers, AppState};
use crate::auth::AuthUser;

pub(super) async fn get_ebooks(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    match db::library_from_db_with_total_combined(
        &state.pool,
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    )
    .await
    {
        Ok((library, total)) => with_pagination_headers(Json(library).into_response(), total),
        Err(error) => internal("read books", error),
    }
}

pub(super) async fn get_ebook_by_uuid(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::get_book_by_uuid(&state.pool, &uuid).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read book", error),
    }
}

pub(super) async fn get_ebook_file(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    let path = match db::book_file_path(&state.pool, id, "EPUB").await {
        Ok(Some(p)) => p,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("book_file_path", e),
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/epub+zip"),
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(?path, error = %e, "epub file read failed");
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
    }
}

pub(super) async fn get_library(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => {
            let contents = scanner::scan_libraries(
                settings.ebook_library_path.as_deref(),
                settings.audiobook_library_path.as_deref(),
            );
            Json(contents).into_response()
        }
        Err(error) => internal("read settings", error),
    }
}

#[cfg(test)]
mod tests;
