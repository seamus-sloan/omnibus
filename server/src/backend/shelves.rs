//! Shelf REST handlers for the mobile client (`/api/shelves*`).
//!
//! List the caller's visible shelves, create/update/delete a shelf, edit
//! hand-picked membership, and preview an unsaved smart rule. Visibility is
//! enforced per request: a shelf is visible to its owner, any admin, or —
//! when public — everyone; mutations require owner-or-admin. Web clients use
//! the parallel `/api/rpc/shelves*` server functions.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, shelves::ShelfError};
use omnibus_shared::{
    CreateShelfRequest, RulePreviewRequest, Shelf, SortDir, SortKey, UpdateShelfRequest,
};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// `GET /api/shelves` — every shelf the caller can see, with live counts.
pub(super) async fn list_shelves(user: AuthUser, State(state): State<AppState>) -> Response {
    match db::list_visible_shelves(&state.pool, user.id, user.is_admin).await {
        Ok(shelves) => Json(shelves).into_response(),
        Err(e) => map_err(e),
    }
}

/// `POST /api/shelves` — create a shelf owned by the caller.
pub(super) async fn create_shelf(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CreateShelfRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
    }
    match db::create_shelf(&state.pool, user.id, &req).await {
        Ok(shelf) => (StatusCode::CREATED, Json(shelf)).into_response(),
        Err(e) => map_err(e),
    }
}

/// `GET /api/shelves/{id}` — one shelf's detail (view check).
pub(super) async fn get_shelf(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match load_for_view(&state, id, &user).await {
        Ok(shelf) => Json(shelf).into_response(),
        Err(resp) => resp,
    }
}

/// `GET /api/shelves/{id}/page?sort=&dir=` — the shelf's member books.
pub(super) async fn get_shelf_page(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<PageQuery>,
) -> Response {
    let shelf = match load_for_view(&state, id, &user).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match db::shelf_page(&state.pool, &shelf, q.sort_key(), q.sort_dir()).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => map_err(e),
    }
}

/// `PATCH /api/shelves/{id}` — partial update (owner or admin).
pub(super) async fn update_shelf(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateShelfRequest>,
) -> Response {
    if let Err(msg) = req.validate() {
        return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
    }
    if let Err(resp) = load_for_edit(&state, id, &user).await {
        return resp;
    }
    match db::update_shelf(&state.pool, id, &req).await {
        Ok(shelf) => Json(shelf).into_response(),
        Err(e) => map_err(e),
    }
}

/// `DELETE /api/shelves/{id}` — delete a shelf (owner or admin).
pub(super) async fn delete_shelf(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = load_for_edit(&state, id, &user).await {
        return resp;
    }
    match db::delete_shelf(&state.pool, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

/// `POST /api/shelves/{id}/books` — append books to a hand-picked shelf.
pub(super) async fn add_shelf_books(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<AddBooksRequest>,
) -> Response {
    if let Err(resp) = load_for_edit(&state, id, &user).await {
        return resp;
    }
    match db::add_books(&state.pool, id, &req.book_uuids, user.id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

/// `DELETE /api/shelves/{id}/books/{uuid}` — remove a book from a shelf.
pub(super) async fn remove_shelf_book(
    user: AuthUser,
    State(state): State<AppState>,
    Path((id, uuid)): Path<(i64, String)>,
) -> Response {
    if let Err(resp) = load_for_edit(&state, id, &user).await {
        return resp;
    }
    match db::remove_book(&state.pool, id, &uuid).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

/// `POST /api/shelves/preview` — evaluate an unsaved smart rule.
pub(super) async fn preview_rule(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<RulePreviewRequest>,
) -> Response {
    for rule in &req.rules {
        if let Err(msg) = rule.validate() {
            return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
        }
    }
    match db::preview_rule(&state.pool, user.id, req.match_mode, &req.rules).await {
        Ok(preview) => Json(preview).into_response(),
        Err(e) => map_err(e),
    }
}

// --- helpers ---------------------------------------------------------------

/// Load a shelf and enforce the view rule (owner, admin, or public). Both an
/// unknown id and a shelf the caller can't see return **404** — existence is
/// deliberately not leaked to non-viewers.
async fn load_for_view(state: &AppState, id: i64, user: &AuthUser) -> Result<Shelf, Response> {
    let shelf = fetch(state, id).await?;
    if db::can_view(&shelf, user.id, user.is_admin) {
        Ok(shelf)
    } else {
        Err((StatusCode::NOT_FOUND, "shelf not found").into_response())
    }
}

/// Load a shelf and enforce the edit rule (owner or admin). A viewer who can see
/// but not edit gets 403; an unknown id gets 404.
async fn load_for_edit(state: &AppState, id: i64, user: &AuthUser) -> Result<Shelf, Response> {
    let shelf = fetch(state, id).await?;
    if db::can_edit(&shelf, user.id, user.is_admin) {
        Ok(shelf)
    } else if db::can_view(&shelf, user.id, user.is_admin) {
        Err((StatusCode::FORBIDDEN, "not your shelf").into_response())
    } else {
        Err((StatusCode::NOT_FOUND, "shelf not found").into_response())
    }
}

async fn fetch(state: &AppState, id: i64) -> Result<Shelf, Response> {
    match db::get_shelf(&state.pool, id).await {
        Ok(Some(shelf)) => Ok(shelf),
        Ok(None) => Err((StatusCode::NOT_FOUND, "shelf not found").into_response()),
        Err(e) => Err(map_err(e)),
    }
}

/// Map a [`ShelfError`] to its HTTP response.
fn map_err(e: ShelfError) -> Response {
    match e {
        ShelfError::NotFound => (StatusCode::NOT_FOUND, "shelf not found").into_response(),
        ShelfError::NameTaken => (
            StatusCode::CONFLICT,
            "a shelf with that name already exists",
        )
            .into_response(),
        ShelfError::BookNotFound => (StatusCode::NOT_FOUND, "book not found").into_response(),
        ShelfError::InvalidRule(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response(),
        ShelfError::Sqlx(e) => internal("shelf operation", e),
    }
}

/// Body for `POST /api/shelves/{id}/books`.
#[derive(serde::Deserialize)]
pub(super) struct AddBooksRequest {
    pub book_uuids: Vec<String>,
}

/// Query string for the shelf page (`?sort=&dir=`).
#[derive(serde::Deserialize)]
pub(super) struct PageQuery {
    sort: Option<String>,
    dir: Option<String>,
}

impl PageQuery {
    fn sort_key(&self) -> SortKey {
        self.sort
            .as_deref()
            .and_then(SortKey::from_wire)
            .unwrap_or(SortKey::NewestAdded)
    }

    fn sort_dir(&self) -> SortDir {
        match self.dir.as_deref() {
            Some("asc") => SortDir::Asc,
            Some("desc") => SortDir::Desc,
            _ => SortDir::Desc,
        }
    }
}

#[cfg(test)]
mod tests;
