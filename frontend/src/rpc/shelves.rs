//! Shelves — smart + hand-picked library subsets: listing, detail,
//! CRUD, member paging, and the smart-rule live preview.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    CreateShelfRequest, MatchMode, RulePreview, Shelf, ShelfPage, ShelfRule, ShelfSummary, SortDir,
    SortKey, UpdateShelfRequest,
};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Map a `ShelfError` to a client-facing `ServerFnError`. The typed variants
/// (`NotFound`, `NameTaken`, `BookNotFound`, `InvalidRule`) already carry a
/// safe, specific message the UI renders directly; only the opaque `Sqlx`
/// variant gets genericized via `internal_rpc_error`.
#[cfg(feature = "server")]
fn map_shelf_error(context: &'static str, e: db::ShelfError) -> ServerFnError {
    match e {
        db::ShelfError::Sqlx(inner) => internal_rpc_error(context, inner),
        other => ServerFnError::new(other.to_string()),
    }
}

/// Load a shelf and enforce the view rule (owner, admin, or public). A hidden
/// or unknown shelf reports "not found" so existence isn't leaked.
#[cfg(feature = "server")]
async fn shelf_for_view(pool: &sqlx::SqlitePool, id: i64, user: &AuthUser) -> Result<Shelf> {
    let Some(shelf) = db::get_shelf(pool, id)
        .await
        .map_err(|e| map_shelf_error("get shelf", e))?
    else {
        return Err(ServerFnError::new("shelf not found").into());
    };
    if db::can_view(&shelf, user.id, user.is_admin) {
        Ok(shelf)
    } else {
        Err(ServerFnError::new("shelf not found").into())
    }
}

/// Load a shelf and enforce the edit rule (owner or admin).
#[cfg(feature = "server")]
async fn shelf_for_edit(pool: &sqlx::SqlitePool, id: i64, user: &AuthUser) -> Result<Shelf> {
    let shelf = shelf_for_view(pool, id, user).await?;
    if db::can_edit(&shelf, user.id, user.is_admin) {
        Ok(shelf)
    } else {
        Err(ServerFnError::new("not your shelf").into())
    }
}

/// Every shelf the caller can see, with live book counts, for the rail.
#[get("/api/rpc/shelves", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_shelves() -> Result<Vec<ShelfSummary>> {
    Ok(db::list_visible_shelves(&pool.0, user.id, user.is_admin)
        .await
        .map_err(|e| map_shelf_error("list shelves", e))?)
}

/// One shelf's full detail (view check).
#[post("/api/rpc/shelves/get", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_shelf(id: i64) -> Result<Shelf> {
    shelf_for_view(&pool.0, id, &user).await
}

/// Create a shelf owned by the caller.
#[post("/api/rpc/shelves/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_shelf(req: CreateShelfRequest) -> Result<Shelf> {
    if let Err(e) = req.validate() {
        return Err(ServerFnError::new(e).into());
    }
    Ok(db::create_shelf(&pool.0, user.id, &req)
        .await
        .map_err(|e| map_shelf_error("create shelf", e))?)
}

/// Partial update (owner or admin).
#[post("/api/rpc/shelves/update", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_shelf(id: i64, req: UpdateShelfRequest) -> Result<Shelf> {
    if let Err(e) = req.validate() {
        return Err(ServerFnError::new(e).into());
    }
    shelf_for_edit(&pool.0, id, &user).await?;
    Ok(db::update_shelf(&pool.0, id, &req)
        .await
        .map_err(|e| map_shelf_error("update shelf", e))?)
}

/// Delete a shelf (owner or admin).
#[post("/api/rpc/shelves/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_shelf(id: i64) -> Result<()> {
    shelf_for_edit(&pool.0, id, &user).await?;
    Ok(db::delete_shelf(&pool.0, id)
        .await
        .map_err(|e| map_shelf_error("delete shelf", e))?)
}

/// The shelf's member books (view check). Smart shelves honor `sort_key`/`dir`.
#[post("/api/rpc/shelves/page", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_shelf_page(
    id: i64,
    sort_key: SortKey,
    sort_dir: SortDir,
) -> Result<ShelfPage> {
    let shelf = shelf_for_view(&pool.0, id, &user).await?;
    Ok(db::shelf_page(&pool.0, &shelf, sort_key, sort_dir)
        .await
        .map_err(|e| map_shelf_error("shelf page", e))?)
}

/// Append books to a hand-picked shelf (owner or admin).
#[post("/api/rpc/shelves/add-books", pool: PoolExt, user: AuthUser)]
pub async fn rpc_add_shelf_books(id: i64, book_uuids: Vec<String>) -> Result<()> {
    shelf_for_edit(&pool.0, id, &user).await?;
    Ok(db::add_books(&pool.0, id, &book_uuids, user.id)
        .await
        .map_err(|e| map_shelf_error("add shelf books", e))?)
}

/// Remove a book from a hand-picked shelf (owner or admin).
#[post("/api/rpc/shelves/remove-book", pool: PoolExt, user: AuthUser)]
pub async fn rpc_remove_shelf_book(id: i64, book_uuid: String) -> Result<()> {
    shelf_for_edit(&pool.0, id, &user).await?;
    Ok(db::remove_book(&pool.0, id, &book_uuid)
        .await
        .map_err(|e| map_shelf_error("remove shelf book", e))?)
}

/// Evaluate an unsaved smart rule for the create-modal live preview.
#[post("/api/rpc/shelves/preview", pool: PoolExt, user: AuthUser)]
pub async fn rpc_preview_shelf_rule(
    match_mode: MatchMode,
    rules: Vec<ShelfRule>,
) -> Result<RulePreview> {
    for rule in &rules {
        if let Err(e) = rule.validate() {
            return Err(ServerFnError::new(e).into());
        }
    }
    Ok(db::preview_rule(&pool.0, user.id, match_mode, &rules)
        .await
        .map_err(|e| map_shelf_error("preview shelf rule", e))?)
}
