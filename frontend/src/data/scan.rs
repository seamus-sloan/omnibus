//! Scan-flow transport: resolve a scanned/typed ISBN and the check-in /
//! add-physical-only / wishlist writes, for mobile (`/api/scan/*` via reqwest)
//! and web/SSR (RPC server functions in `crate::rpc`). Each function's public
//! signature is shared across platforms; the `#[cfg]` gates carry the split.

use omnibus_shared::{
    AddPhysicalOnlyRequest, BookRef, CheckInRequest, ResolveRequest, ScanOutcome,
    WishlistAddRequest,
};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

// Mobile (REST).

#[cfg(feature = "mobile")]
async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
    server_url: &str,
    path: &str,
    body: &B,
) -> Result<T, DataError> {
    let response = with_bearer(http_client().post(format!("{server_url}{path}")).json(body))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<T>().await?)
}

/// POST `/api/scan/resolve` — resolve an ISBN down the matching ladder.
#[cfg(feature = "mobile")]
pub async fn resolve_scan(server_url: &str, req: ResolveRequest) -> Result<ScanOutcome, DataError> {
    post_json(server_url, "/api/scan/resolve", &req).await
}

/// POST `/api/scan/check-in` — check in a physical copy of a library book.
#[cfg(feature = "mobile")]
pub async fn check_in(server_url: &str, req: CheckInRequest) -> Result<BookRef, DataError> {
    post_json(server_url, "/api/scan/check-in", &req).await
}

/// POST `/api/scan/physical-only` — add a physical-only book from external meta.
#[cfg(feature = "mobile")]
pub async fn add_physical_only(
    server_url: &str,
    req: AddPhysicalOnlyRequest,
) -> Result<BookRef, DataError> {
    post_json(server_url, "/api/scan/physical-only", &req).await
}

/// POST `/api/scan/wishlist` — add a book to the caller's physical wishlist.
#[cfg(feature = "mobile")]
pub async fn wishlist_add(server_url: &str, req: WishlistAddRequest) -> Result<BookRef, DataError> {
    post_json(server_url, "/api/scan/wishlist", &req).await
}

// Web (RPC).

/// Resolve a scanned/typed ISBN.
#[cfg(not(feature = "mobile"))]
pub async fn resolve_scan(
    _server_url: &str,
    req: ResolveRequest,
) -> Result<ScanOutcome, DataError> {
    crate::rpc::rpc_resolve_scan(req)
        .await
        .map_err(note_server_fn_err)
}

/// Check in a physical copy of a library book.
#[cfg(not(feature = "mobile"))]
pub async fn check_in(_server_url: &str, req: CheckInRequest) -> Result<BookRef, DataError> {
    crate::rpc::rpc_check_in(req)
        .await
        .map_err(note_server_fn_err)
}

/// Add a physical-only book from external meta.
#[cfg(not(feature = "mobile"))]
pub async fn add_physical_only(
    _server_url: &str,
    req: AddPhysicalOnlyRequest,
) -> Result<BookRef, DataError> {
    crate::rpc::rpc_add_physical_only(req)
        .await
        .map_err(note_server_fn_err)
}

/// Add a book to the caller's physical wishlist.
#[cfg(not(feature = "mobile"))]
pub async fn wishlist_add(
    _server_url: &str,
    req: WishlistAddRequest,
) -> Result<BookRef, DataError> {
    crate::rpc::rpc_wishlist_add(req)
        .await
        .map_err(note_server_fn_err)
}
