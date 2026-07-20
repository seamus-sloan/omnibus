//! Physical Check-In scan flow: resolve a scanned/typed ISBN and the
//! check-in / add-physical-only / wishlist write endpoints. Mobile uses the
//! analogous REST routes in `server::backend::scan`.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{
    AddPhysicalOnlyRequest, BookRef, CheckInRequest, ResolveRequest, ScanOutcome,
    WishlistAddRequest,
};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Map a scan-flow error to a client-facing `ServerFnError`: user-actionable
/// cases (bad ISBN, unknown book, missing wishlist target) carry a specific
/// message; provider/DB failures are genericized + logged.
#[cfg(feature = "server")]
fn map_scan_err(e: db::ScanError) -> ServerFnError {
    match &e {
        db::ScanError::Isbn(inner) => ServerFnError::new(inner.to_string()),
        db::ScanError::Physical(db::PhysicalError::BookNotFound) => {
            ServerFnError::new("book not found")
        }
        db::ScanError::MissingWishlistTarget => ServerFnError::new(e.to_string()),
        _ => internal_rpc_error("scan", e),
    }
}

/// Resolve a scanned/typed ISBN down the matching ladder. Any authenticated
/// user may resolve; ownership is library-wide.
#[post("/api/rpc/scan/resolve", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_resolve_scan(req: ResolveRequest) -> Result<ScanOutcome> {
    let config = db::MetadataLookupConfig::live();
    Ok(db::resolve_scan(&pool.0, &req.isbn, &config)
        .await
        .map_err(map_scan_err)?)
}

/// Check in a physical copy of a book already in the library (fulfills every
/// user's wishlist for it).
#[post("/api/rpc/scan/check-in", pool: PoolExt, user: AuthUser)]
pub async fn rpc_check_in(req: CheckInRequest) -> Result<BookRef> {
    let copy = db::add_physical_copy(
        &pool.0,
        &req.book_uuid,
        req.isbn.as_deref(),
        Some(user.id),
        req.note.as_deref(),
    )
    .await
    .map_err(|e| map_scan_err(e.into()))?;
    Ok(BookRef {
        book_uuid: copy.book_uuid,
    })
}

/// Add a physical-only book (not in the library) from resolved external meta.
#[post("/api/rpc/scan/physical-only", pool: PoolExt, user: AuthUser)]
pub async fn rpc_add_physical_only(req: AddPhysicalOnlyRequest) -> Result<BookRef> {
    let book_uuid = db::add_physical_only(&pool.0, &req.meta, req.note.as_deref(), Some(user.id))
        .await
        .map_err(map_scan_err)?;
    Ok(BookRef { book_uuid })
}

/// Add a book to the caller's physical wishlist — an existing library book or a
/// new fileless book from external meta.
#[post("/api/rpc/scan/wishlist", pool: PoolExt, user: AuthUser)]
pub async fn rpc_wishlist_add(req: WishlistAddRequest) -> Result<BookRef> {
    let book_uuid = db::wishlist_add(
        &pool.0,
        user.id,
        req.book_uuid.as_deref(),
        req.meta.as_ref(),
        req.source,
    )
    .await
    .map_err(map_scan_err)?;
    Ok(BookRef { book_uuid })
}
