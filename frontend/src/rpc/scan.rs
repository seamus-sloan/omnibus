//! Physical Check-In scan flow: resolve a scanned/typed ISBN and the
//! check-in / add-physical-only / wishlist write endpoints. Mobile uses the
//! analogous REST routes in `server::backend::scan`.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    AddPhysicalOnlyRequest, BookRef, CheckInRequest, GoogleBooksKeyStatus, ResolveMetaRequest,
    ResolveRequest, ScanOutcome, ScanSearchRequest, ScanSearchResponse, WishlistAddRequest,
};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, AuthUser, PoolExt};

/// The provider ladder's config for this instance: saved settings keys win
/// over the `GOOGLE_BOOKS_API_KEY` / `HARDCOVER_API_KEY` env vars, and a
/// provider with no key is simply one the ladder skips.
#[cfg(feature = "server")]
async fn provider_config(pool: &sqlx::SqlitePool) -> Result<db::MetadataLookupConfig> {
    Ok(db::MetadataLookupConfig::live(
        db::provider_keys(pool)
            .await
            .map_err(|e| internal_rpc_error("resolve provider keys", e))?,
    ))
}

/// Map a scan-flow error to a client-facing `ServerFnError`: user-actionable
/// cases (bad ISBN, unknown book, missing wishlist target, an unreachable
/// provider) carry a specific message; DB failures are genericized + logged.
#[cfg(feature = "server")]
fn map_scan_err(e: db::ScanError) -> ServerFnError {
    match &e {
        db::ScanError::Isbn(inner) => ServerFnError::new(inner.to_string()),
        db::ScanError::Physical(db::PhysicalError::BookNotFound) => {
            ServerFnError::new("book not found")
        }
        db::ScanError::MissingWishlistTarget => ServerFnError::new(e.to_string()),
        // A provider outage is not an Omnibus bug — surface the "try again
        // later" sentence rather than the generic internal-error text, but
        // still log the provider's own message.
        db::ScanError::Lookup(inner @ db::MetadataLookupError::Provider(_)) => {
            tracing::warn!(error = ?inner, "metadata provider unavailable");
            ServerFnError::new(inner.to_string())
        }
        _ => internal_rpc_error("scan", e),
    }
}

/// Resolve a scanned/typed ISBN down the matching ladder. Any authenticated
/// user may resolve; ownership is library-wide.
#[post("/api/rpc/scan/resolve", pool: PoolExt, user: AuthUser)]
pub async fn rpc_resolve_scan(req: ResolveRequest) -> Result<ScanOutcome> {
    if let Err(msg) = req.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    let config = provider_config(&pool.0).await?;
    Ok(db::resolve_scan(&pool.0, user.id, &req.isbn, &config)
        .await
        .map_err(map_scan_err)?)
}

/// Search the providers by title text — the fallback when an ISBN resolves to
/// `Unresolved`. Any authenticated user may search.
#[post("/api/rpc/scan/search", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_scan_search(req: ScanSearchRequest) -> Result<ScanSearchResponse> {
    if let Err(msg) = req.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    let config = provider_config(&pool.0).await?;
    let results = db::search_provider_by_title(&config, req.query.trim())
        .await
        .map_err(|e| map_scan_err(e.into()))?;
    Ok(ScanSearchResponse { results })
}

/// Resolve a picked title-search candidate against the library — the ladder's
/// library rungs applied to metadata already in hand, no provider ISBN lookup.
#[post("/api/rpc/scan/resolve-meta", pool: PoolExt, user: AuthUser)]
pub async fn rpc_resolve_scan_meta(req: ResolveMetaRequest) -> Result<ScanOutcome> {
    if let Err(msg) = req.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    let config = provider_config(&pool.0).await?;
    Ok(db::resolve_meta(&pool.0, user.id, &req.meta, &config)
        .await
        .map_err(map_scan_err)?)
}

// This key's RPC pair mirrors the Hardcover pair in `rpc/settings.rs`; kept per
// feature module rather than unified — the shared logic already lives once in
// db's `SecretKeySpec`, so these stay thin route wrappers.
/// Admin-only: masked status of the server-wide Google Books key for Settings.
/// Never returns the raw key.
#[get("/api/rpc/google-books-key", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_google_books_key() -> Result<GoogleBooksKeyStatus> {
    Ok(db::google_books_key_status(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get google books key status", e))?)
}

/// Admin-only: save (or clear, with `None`/blank) the Google Books key in
/// settings. Returns the new masked status — never echoes the raw key. Rejects
/// keys longer than `GOOGLE_BOOKS_API_KEY_MAX_LEN` before the KV write,
/// surfacing the validation message via `ServerFnError`.
#[post("/api/rpc/google-books-key", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_set_google_books_key(key: Option<String>) -> Result<GoogleBooksKeyStatus> {
    match db::set_google_books_api_key(&pool.0, key.as_deref()).await {
        Ok(()) => Ok(db::google_books_key_status(&pool.0)
            .await
            .map_err(|e| internal_rpc_error("get google books key status", e))?),
        Err(db::SettingsError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("set google books key", e).into()),
    }
}

/// Whether a server-wide Google Books key is configured. Any authenticated
/// user may read this non-sensitive flag; it drives the check-in scan
/// screen's provider note, without needing the admin-only masked-status
/// [`rpc_get_google_books_key`] (mirrors `rpc_hardcover_configured` in
/// `rpc::summary`).
#[post("/api/rpc/scan/google-books-configured", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_google_books_configured() -> Result<bool> {
    let status = db::google_books_key_status(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get google books key status", e))?;
    Ok(status.configured)
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

// `server`-gated: exercises the five-way error mapping directly, no DB
// needed. CI runs this via `cargo test -p omnibus-frontend --features server`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{db, map_scan_err};

    #[test]
    fn map_scan_err_surfaces_the_isbn_validation_message() {
        let e = db::ScanError::Isbn(omnibus_shared::IsbnError::InvalidLength(3));
        let err = map_scan_err(e);
        assert!(err.to_string().contains("ISBN must be 10 or 13 digits"));
    }

    #[test]
    fn map_scan_err_maps_physical_book_not_found_to_a_book_not_found_message() {
        let e = db::ScanError::Physical(db::PhysicalError::BookNotFound);
        let err = map_scan_err(e);
        assert!(err.to_string().contains("book not found"));
    }

    #[test]
    fn map_scan_err_surfaces_the_missing_wishlist_target_message() {
        let e = db::ScanError::MissingWishlistTarget;
        let err = map_scan_err(e);
        assert!(err
            .to_string()
            .contains("wishlist add requires a book_uuid or meta"));
    }

    #[test]
    fn map_scan_err_surfaces_the_provider_outage_message_and_logs_a_warning() {
        // `.into()` targets `anyhow::Error` via `MetadataLookupError::Provider`'s
        // field type without this crate needing a direct `anyhow` dependency.
        let io_err = std::io::Error::other("provider timed out");
        let e = db::ScanError::Lookup(db::MetadataLookupError::Provider(io_err.into()));
        let err = map_scan_err(e);
        // The caller-facing sentence is deliberately generic, not the raw
        // provider error — see `MetadataLookupError::Provider`'s doc comment.
        // The `tracing::warn!` side effect on this branch has no return
        // value to assert on; the message mapping is what a client observes.
        assert!(err.to_string().contains("try again later"));
    }

    #[test]
    fn map_scan_err_genericizes_an_opaque_db_failure() {
        let e = db::ScanError::Sqlx(sqlx::Error::RowNotFound);
        let err = map_scan_err(e);
        assert!(err.to_string().contains("internal server error"));
    }
}
